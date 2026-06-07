use super::WorkloadManager;
use super::runtime::{RuntimeTaskState, RuntimeTaskStatus};
use super::types::{ActualState, Workload, WorkloadResult, duration_secs, now_unix_secs};

const RUNTIME_UNKNOWN_ERROR: &str = "runtime task state remained unknown";

pub(crate) async fn run_once(manager: &WorkloadManager) -> WorkloadResult<()> {
    let workloads = manager.storage.list_watchable()?;

    for workload in workloads {
        if let Err(error) = update_workload(manager, &workload).await {
            eprintln!(
                "billow-agent: workload watch for {} failed: {error}",
                workload.id
            );
        }
    }

    Ok(())
}

async fn update_workload(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    let Some(runtime_task_id) = workload.runtime_task_id.clone() else {
        let current = manager.storage.get(&workload.id)?;
        apply_not_found(manager, &current)?;
        return Ok(());
    };

    let status = manager.runtime.inspect(&runtime_task_id).await?;
    let current = manager.storage.get(&workload.id)?;
    if current.runtime_task_id.as_deref() != Some(runtime_task_id.as_str())
        || !is_watchable(current.actual_state)
    {
        return Ok(());
    }

    match status {
        Some(status) => apply_status(manager, &current, status),
        None => apply_not_found(manager, &current),
    }
}

fn apply_not_found(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    match workload.actual_state {
        ActualState::Stopping => {
            manager.storage.compare_and_set_actual(
                &workload.id,
                ActualState::Stopping,
                ActualState::Stopped,
                None,
                None,
            )?;
        }
        ActualState::Starting | ActualState::Running => {
            let error = workload.kind.policy().missing_runtime_task_error();
            manager.storage.compare_and_set_actual(
                &workload.id,
                workload.actual_state,
                ActualState::Failed,
                None,
                Some(error),
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn apply_status(
    manager: &WorkloadManager,
    workload: &Workload,
    status: RuntimeTaskStatus,
) -> WorkloadResult<()> {
    match status.state {
        RuntimeTaskState::Created => {
            if workload.actual_state != ActualState::Stopping {
                manager.storage.compare_and_set_actual(
                    &workload.id,
                    workload.actual_state,
                    ActualState::Starting,
                    None,
                    None,
                )?;
            }
        }
        RuntimeTaskState::Running => {
            if workload.actual_state != ActualState::Stopping {
                manager.storage.compare_and_set_actual(
                    &workload.id,
                    workload.actual_state,
                    ActualState::Running,
                    None,
                    None,
                )?;
            }
        }
        RuntimeTaskState::Stopped => {
            manager.storage.set_stopped_outcome(
                &workload.id,
                workload.actual_state,
                status.exit_code,
            )?;
        }
        RuntimeTaskState::Unknown => apply_unknown(manager, workload)?,
    }

    Ok(())
}

fn apply_unknown(manager: &WorkloadManager, workload: &Workload) -> WorkloadResult<()> {
    if !matches!(
        workload.actual_state,
        ActualState::Starting | ActualState::Running
    ) {
        return Ok(());
    }

    let now = now_unix_secs();
    let unknown_since = workload.runtime_unknown_since_unix_secs.unwrap_or(now);
    if workload.runtime_unknown_since_unix_secs.is_none() {
        manager
            .storage
            .mark_runtime_unknown(&workload.id, workload.actual_state, now)?;
        return Ok(());
    }

    if now.saturating_sub(unknown_since) < duration_secs(WorkloadManager::runtime_unknown_grace()) {
        return Ok(());
    }

    let error = format!(
        "{RUNTIME_UNKNOWN_ERROR} for {} seconds",
        duration_secs(WorkloadManager::runtime_unknown_grace())
    );
    manager.storage.compare_and_set_actual(
        &workload.id,
        workload.actual_state,
        ActualState::Failed,
        None,
        Some(&error),
    )?;
    Ok(())
}

fn is_watchable(actual_state: ActualState) -> bool {
    matches!(
        actual_state,
        ActualState::Starting | ActualState::Running | ActualState::Stopping
    )
}
