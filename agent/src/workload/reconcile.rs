use super::WorkloadManager;
use super::runtime::{RuntimeCleanupMode, RuntimeStartRequest, RuntimeStopMode, RuntimeTaskState};
use super::storage::{CLEANUP_RETRY_BACKOFF_CAP_SECS, CLEANUP_RETRY_BACKOFF_SECS};
use super::types::{
    ActualState, CleanupState, DesiredState, Workload, WorkloadResult, WorkloadRun, duration_secs,
    now_unix_secs,
};
use uuid::Uuid;

const MAX_CLEANUP_ATTEMPTS: i64 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CleanupOutcome {
    Done,
    Pending,
    Failed,
}

pub(crate) async fn run_once(manager: &WorkloadManager) -> WorkloadResult<()> {
    let workloads = manager.storage.list_for_reconcile()?;

    for workload in workloads {
        if let Err(error) = reconcile_workload(manager, &workload).await {
            eprintln!(
                "billow-agent: workload reconcile for {} failed: {error}",
                workload.id
            );
        }
    }

    if let Err(error) = prune_cleaned_runs(manager).await {
        eprintln!("billow-agent: workload run pruning failed: {error}");
    }
    Ok(())
}

async fn reconcile_workload(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    // Once a workload is Stopping, a stop is already in flight (SIGTERM sent).
    // A container mid-shutdown may be half-alive (e.g. HTTP server closed but an
    // event-loop teardown aborted on a bug), so we never cancel an in-progress stop
    // even if desired flips back to Running — we complete the stop and let the next
    // reconcile restart cleanly.
    if workload.actual_state == ActualState::Stopping {
        return escalate_stop(manager, workload).await;
    }

    match workload.desired_state {
        DesiredState::Running => reconcile_desired_running(manager, workload).await,
        DesiredState::Stopped => reconcile_desired_stopped(manager, workload).await,
        DesiredState::Deleted => reconcile_desired_deleted(manager, workload).await,
    }
}

async fn reconcile_desired_running(
    manager: &WorkloadManager,
    workload: &Workload,
) -> WorkloadResult<()> {
    match workload.actual_state {
        ActualState::Accepted => start_workload(manager, workload).await,
        ActualState::Creating => recover_creating(manager, workload).await,
        ActualState::Stopped | ActualState::Failed
            if workload
                .kind
                .policy()
                .should_restart_after_terminal(workload.actual_state) =>
        {
            let cleanup = cleanup_latest_run(manager, workload).await?;
            if !restart_backoff_ready(manager, workload)? {
                return Ok(());
            }
            if cleanup == CleanupOutcome::Done {
                start_workload(manager, workload).await?;
            }
            Ok(())
        }
        ActualState::Stopped | ActualState::Failed => {
            cleanup_latest_run(manager, workload).await?;
            Ok(())
        }
        ActualState::Running => {
            reset_restart_backoff_after_stable_run(manager, workload)?;
            Ok(())
        }
        ActualState::Starting | ActualState::Stopping => Ok(()),
        ActualState::Deleted => Ok(()),
    }
}

async fn reconcile_desired_stopped(
    manager: &WorkloadManager,
    workload: &Workload,
) -> WorkloadResult<()> {
    match workload.actual_state {
        ActualState::Accepted => {
            manager.storage.compare_and_set_actual(
                &workload.id,
                ActualState::Accepted,
                ActualState::Stopped,
                None,
                None,
            )?;
            Ok(())
        }
        ActualState::Creating | ActualState::Starting | ActualState::Running => {
            enter_stopping(manager, workload).await
        }
        ActualState::Stopped | ActualState::Failed => {
            cleanup_latest_run(manager, workload).await?;
            Ok(())
        }
        ActualState::Stopping | ActualState::Deleted => Ok(()),
    }
}

async fn reconcile_desired_deleted(
    manager: &WorkloadManager,
    workload: &Workload,
) -> WorkloadResult<()> {
    match workload.actual_state {
        ActualState::Accepted => {
            if cleanup_latest_run(manager, workload).await? != CleanupOutcome::Pending {
                manager.storage.compare_and_set_actual(
                    &workload.id,
                    ActualState::Accepted,
                    ActualState::Deleted,
                    workload.exit_code,
                    workload.error.as_deref(),
                )?;
            }
            Ok(())
        }
        ActualState::Creating | ActualState::Starting | ActualState::Running => {
            enter_stopping(manager, workload).await
        }
        ActualState::Stopped | ActualState::Failed => {
            if cleanup_latest_run(manager, workload).await? != CleanupOutcome::Pending {
                manager.storage.compare_and_set_actual(
                    &workload.id,
                    workload.actual_state,
                    ActualState::Deleted,
                    workload.exit_code,
                    workload.error.as_deref(),
                )?;
            }
            Ok(())
        }
        ActualState::Stopping | ActualState::Deleted => Ok(()),
    }
}

async fn start_workload(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    let runtime_task_id = runtime_task_id(workload);

    if !manager
        .storage
        .begin_start(&workload.id, workload.actual_state, &runtime_task_id)?
    {
        return Ok(());
    }

    let start_result = manager
        .runtime
        .start(RuntimeStartRequest {
            workload_id: workload.id.clone(),
            runtime_task_id: runtime_task_id.clone(),
            image: workload.image.clone(),
        })
        .await;

    match start_result {
        Ok(_) => {
            manager.storage.compare_and_set_actual(
                &workload.id,
                ActualState::Creating,
                ActualState::Starting,
                None,
                None,
            )?;
        }
        Err(error) => {
            if let Some(run) = manager.storage.latest_run(&workload.id)? {
                if let Err(cleanup_error) =
                    cleanup_runtime_run(manager, &run, RuntimeCleanupMode::PreserveLogs).await
                {
                    eprintln!(
                        "billow-agent: cleanup after failed start for {} failed: {cleanup_error}",
                        workload.id
                    );
                }
            }
            manager.storage.compare_and_set_actual(
                &workload.id,
                ActualState::Creating,
                ActualState::Failed,
                None,
                Some(&error.to_string()),
            )?;
        }
    }
    Ok(())
}

async fn recover_creating(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    let Some(run) = manager.storage.latest_run(&workload.id)? else {
        manager.storage.compare_and_set_actual(
            &workload.id,
            ActualState::Creating,
            ActualState::Accepted,
            None,
            None,
        )?;
        return Ok(());
    };

    let status = match manager.runtime.inspect(&run.runtime_task_id).await {
        Ok(status) => status,
        Err(error) => {
            eprintln!(
                "billow-agent: inspect during creating recovery for {} failed: {error}",
                workload.id
            );
            return Ok(());
        }
    };

    let adopt = matches!(
        status,
        Some(status) if matches!(status.state, RuntimeTaskState::Running | RuntimeTaskState::Stopped)
    );
    if adopt {
        manager.storage.compare_and_set_actual(
            &workload.id,
            ActualState::Creating,
            ActualState::Starting,
            None,
            None,
        )?;
        return Ok(());
    }

    match cleanup_runtime_run(manager, &run, RuntimeCleanupMode::RemoveLogs).await? {
        CleanupOutcome::Done => {
            manager.storage.delete_run(&run.runtime_task_id)?;
            manager.storage.compare_and_set_actual(
                &workload.id,
                ActualState::Creating,
                ActualState::Accepted,
                None,
                None,
            )?;
        }
        CleanupOutcome::Pending | CleanupOutcome::Failed => {}
    }
    Ok(())
}

async fn enter_stopping(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    if !manager.storage.compare_and_set_actual(
        &workload.id,
        workload.actual_state,
        ActualState::Stopping,
        None,
        None,
    )? {
        return Ok(());
    }

    let current = manager.storage.get(&workload.id)?;
    send_stop_signal(manager, &current, RuntimeStopMode::Graceful).await;
    Ok(())
}

async fn escalate_stop(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    let Some(runtime_task_id) = workload.runtime_task_id.as_deref() else {
        manager.storage.compare_and_set_actual(
            &workload.id,
            ActualState::Stopping,
            ActualState::Stopped,
            None,
            None,
        )?;
        return Ok(());
    };

    let status = match manager.runtime.inspect(runtime_task_id).await {
        Ok(status) => status,
        Err(error) => {
            eprintln!(
                "billow-agent: inspect during stop escalation for {} failed: {error}",
                workload.id
            );
            return Ok(());
        }
    };

    let terminated = match status {
        None => true,
        Some(status) => status.state == RuntimeTaskState::Stopped,
    };
    if terminated {
        manager.storage.compare_and_set_actual(
            &workload.id,
            ActualState::Stopping,
            ActualState::Stopped,
            None,
            None,
        )?;
        return Ok(());
    }

    let now = now_unix_secs();
    let stopping_since = workload.stopping_since_unix_secs.unwrap_or(now);
    let elapsed_secs = now.saturating_sub(stopping_since);
    let grace_secs = duration_secs(WorkloadManager::stop_grace());
    let kill_secs = duration_secs(WorkloadManager::stop_kill());

    if elapsed_secs < grace_secs {
        send_stop_signal(manager, workload, RuntimeStopMode::Graceful).await;
        return Ok(());
    }

    send_stop_signal(manager, workload, RuntimeStopMode::Force).await;
    if elapsed_secs >= grace_secs.saturating_add(kill_secs) {
        manager.storage.compare_and_set_actual(
            &workload.id,
            ActualState::Stopping,
            ActualState::Stopping,
            None,
            Some("stop escalation stuck: runtime task still alive after SIGKILL"),
        )?;
    }
    Ok(())
}

async fn cleanup_latest_run(
    manager: &WorkloadManager,
    workload: &Workload,
) -> WorkloadResult<CleanupOutcome> {
    let Some(run) = manager.storage.latest_run(&workload.id)? else {
        return Ok(CleanupOutcome::Done);
    };

    match run.cleanup_state {
        CleanupState::Done => return Ok(CleanupOutcome::Done),
        CleanupState::Failed if !cleanup_retry_due(&run) => return Ok(CleanupOutcome::Failed),
        _ => {}
    }

    cleanup_runtime_run(manager, &run, RuntimeCleanupMode::PreserveLogs).await
}

fn cleanup_retry_due(run: &WorkloadRun) -> bool {
    let backoff = CLEANUP_RETRY_BACKOFF_SECS
        .saturating_mul(run.cleanup_attempts)
        .min(CLEANUP_RETRY_BACKOFF_CAP_SECS);
    now_unix_secs().saturating_sub(run.updated_at_unix_secs) >= backoff
}

fn restart_backoff_ready(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<bool> {
    let now = now_unix_secs();
    if let Some(not_before) = workload.restart_not_before_unix_secs {
        return Ok(now >= not_before);
    }

    let delay = restart_backoff_delay_secs(workload.restart_attempts);
    let not_before = now.saturating_add(delay);
    let attempts = workload.restart_attempts.saturating_add(1);
    manager.storage.set_restart_backoff(
        &workload.id,
        workload.actual_state,
        attempts,
        not_before,
    )?;
    Ok(false)
}

fn reset_restart_backoff_after_stable_run(
    manager: &WorkloadManager,
    workload: &Workload,
) -> WorkloadResult<()> {
    if workload.restart_attempts == 0 && workload.restart_not_before_unix_secs.is_none() {
        return Ok(());
    }

    let Some(running_since) = workload.running_since_unix_secs else {
        return Ok(());
    };
    let stable_secs = duration_secs(WorkloadManager::restart_backoff_reset());
    if now_unix_secs().saturating_sub(running_since) >= stable_secs {
        manager.storage.clear_restart_backoff(&workload.id)?;
    }
    Ok(())
}

fn restart_backoff_delay_secs(restart_attempts: i64) -> i64 {
    let initial = duration_secs(WorkloadManager::restart_backoff_initial());
    let cap = duration_secs(WorkloadManager::restart_backoff_cap());
    let mut delay = initial;

    for _ in 0..restart_attempts.max(0) {
        delay = delay.saturating_mul(2).min(cap);
        if delay == cap {
            break;
        }
    }

    delay
}

async fn send_stop_signal(manager: &WorkloadManager, workload: &Workload, mode: RuntimeStopMode) {
    if let Some(runtime_task_id) = workload.runtime_task_id.as_deref() {
        if let Err(error) = manager.runtime.stop(runtime_task_id, mode).await {
            eprintln!(
                "billow-agent: failed to send {mode:?} stop to {}: {error}",
                workload.id
            );
        }
    }
}

async fn cleanup_runtime_run(
    manager: &WorkloadManager,
    run: &WorkloadRun,
    mode: RuntimeCleanupMode,
) -> WorkloadResult<CleanupOutcome> {
    match manager.runtime.cleanup(&run.runtime_task_id, mode).await {
        Ok(()) => {
            manager
                .storage
                .mark_run_cleanup_done(&run.runtime_task_id)?;
            Ok(CleanupOutcome::Done)
        }
        Err(error) => {
            let attempts = run.cleanup_attempts.saturating_add(1);
            let next_state = if attempts >= MAX_CLEANUP_ATTEMPTS {
                CleanupState::Failed
            } else {
                CleanupState::Pending
            };
            manager.storage.record_run_cleanup_failure(
                &run.runtime_task_id,
                next_state,
                attempts,
                &error.to_string(),
            )?;
            eprintln!(
                "billow-agent: cleanup for run {} failed on attempt {}: {error}",
                run.runtime_task_id, attempts
            );
            if next_state == CleanupState::Failed {
                Ok(CleanupOutcome::Failed)
            } else {
                Ok(CleanupOutcome::Pending)
            }
        }
    }
}

async fn prune_cleaned_runs(manager: &WorkloadManager) -> WorkloadResult<()> {
    for run in manager.storage.list_prunable_runs()? {
        if cleanup_runtime_run(manager, &run, RuntimeCleanupMode::RemoveLogs).await?
            == CleanupOutcome::Done
        {
            manager.storage.delete_run(&run.runtime_task_id)?;
        }
    }

    Ok(())
}

fn runtime_task_id(workload: &Workload) -> String {
    let workload_prefix: String = workload
        .id
        .chars()
        .filter(|character| *character != '-')
        .take(20)
        .collect();
    format!("billow-{}-{}", workload_prefix, Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::super::types::WorkloadKind;
    use super::*;

    #[test]
    fn runtime_task_id_fits_linux_hostname_limit() {
        let workload = Workload {
            id: Uuid::new_v4().to_string(),
            kind: WorkloadKind::Service,
            image: String::from("nginx"),
            desired_state: DesiredState::Running,
            actual_state: ActualState::Accepted,
            runtime_task_id: None,
            exit_code: None,
            error: None,
            stopping_since_unix_secs: None,
            runtime_unknown_since_unix_secs: None,
            restart_attempts: 0,
            restart_not_before_unix_secs: None,
            running_since_unix_secs: None,
            created_at_unix_secs: 0,
            updated_at_unix_secs: 0,
        };

        assert!(runtime_task_id(&workload).len() <= 63);
    }
}
