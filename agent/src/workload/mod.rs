mod logs;
mod policy;
mod reconcile;
pub(crate) mod runtime;
pub(crate) mod storage;
pub(crate) mod types;
mod watch;

use self::runtime::{ContainerRuntime, RuntimeLogs};
use self::storage::WorkloadStorage;
use self::types::{
    ActualState, DesiredState, Workload, WorkloadError, WorkloadKind, WorkloadResult,
    env_duration_or_default, env_path_or_default, now_unix_secs,
};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const DEFAULT_DB_PATH: &str = "/var/lib/billow/workloads.sqlite3";
const DB_PATH_ENV: &str = "BILLOW_WORKLOAD_DB_PATH";
const DEFAULT_LOG_LIMIT_BYTES: usize = 1024 * 1024;
const DEFAULT_WATCH_INTERVAL_SECS: u64 = 1;
const DEFAULT_RECONCILE_INTERVAL_SECS: u64 = 2;
const DEFAULT_STOP_GRACE_SECS: u64 = 10;
const DEFAULT_STOP_KILL_SECS: u64 = 10;
const DEFAULT_RUNTIME_UNKNOWN_GRACE_SECS: u64 = 30;
const DEFAULT_RESTART_BACKOFF_INITIAL_SECS: u64 = 5;
const DEFAULT_RESTART_BACKOFF_CAP_SECS: u64 = 300;
const DEFAULT_RESTART_BACKOFF_RESET_SECS: u64 = 600;
const WATCH_INTERVAL_ENV: &str = "BILLOW_WORKLOAD_WATCH_INTERVAL_SECS";
const RECONCILE_INTERVAL_ENV: &str = "BILLOW_WORKLOAD_RECONCILE_INTERVAL_SECS";
const STOP_GRACE_ENV: &str = "BILLOW_WORKLOAD_STOP_GRACE_SECS";
const STOP_KILL_ENV: &str = "BILLOW_WORKLOAD_STOP_KILL_SECS";

#[derive(Clone)]
pub(crate) struct WorkloadManager {
    pub(crate) storage: WorkloadStorage,
    pub(crate) runtime: Arc<dyn ContainerRuntime>,
    pub(crate) log_limit_bytes: usize,
}

impl WorkloadManager {
    pub(crate) fn open(runtime: Arc<dyn ContainerRuntime>) -> WorkloadResult<Self> {
        let storage = WorkloadStorage::open(db_path())?;
        Ok(Self::new(storage, runtime))
    }

    pub(crate) fn new(storage: WorkloadStorage, runtime: Arc<dyn ContainerRuntime>) -> Self {
        Self {
            storage,
            runtime,
            log_limit_bytes: DEFAULT_LOG_LIMIT_BYTES,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_log_limit(mut self, log_limit_bytes: usize) -> Self {
        self.log_limit_bytes = log_limit_bytes;
        self
    }

    pub(crate) fn watch_interval() -> Duration {
        env_duration_or_default(WATCH_INTERVAL_ENV, DEFAULT_WATCH_INTERVAL_SECS)
    }

    pub(crate) fn reconcile_interval() -> Duration {
        env_duration_or_default(RECONCILE_INTERVAL_ENV, DEFAULT_RECONCILE_INTERVAL_SECS)
    }

    pub(crate) fn stop_grace() -> Duration {
        env_duration_or_default(STOP_GRACE_ENV, DEFAULT_STOP_GRACE_SECS)
    }

    pub(crate) fn stop_kill() -> Duration {
        env_duration_or_default(STOP_KILL_ENV, DEFAULT_STOP_KILL_SECS)
    }

    pub(crate) fn runtime_unknown_grace() -> Duration {
        Duration::from_secs(DEFAULT_RUNTIME_UNKNOWN_GRACE_SECS)
    }

    pub(crate) fn restart_backoff_initial() -> Duration {
        Duration::from_secs(DEFAULT_RESTART_BACKOFF_INITIAL_SECS)
    }

    pub(crate) fn restart_backoff_cap() -> Duration {
        Duration::from_secs(DEFAULT_RESTART_BACKOFF_CAP_SECS)
    }

    pub(crate) fn restart_backoff_reset() -> Duration {
        Duration::from_secs(DEFAULT_RESTART_BACKOFF_RESET_SECS)
    }

    pub(crate) async fn submit(
        &self,
        kind: WorkloadKind,
        image: String,
    ) -> WorkloadResult<Workload> {
        let image = image.trim().to_string();
        if image.is_empty() {
            return Err(WorkloadError::invalid_argument(
                "image reference cannot be empty",
            ));
        }

        let desired_state = kind.policy().initial_desired_state();
        let now = now_unix_secs();
        let workload = Workload {
            id: Uuid::new_v4().to_string(),
            kind,
            image,
            desired_state,
            actual_state: ActualState::Accepted,
            runtime_task_id: None,
            container_ip: None,
            exit_code: None,
            error: None,
            stopping_since_unix_secs: None,
            runtime_unknown_since_unix_secs: None,
            restart_attempts: 0,
            restart_not_before_unix_secs: None,
            running_since_unix_secs: None,
            created_at_unix_secs: now,
            updated_at_unix_secs: now,
        };

        self.storage.save(&workload)?;
        self.storage.get(&workload.id)
    }

    pub(crate) fn get(&self, workload_id: &str) -> WorkloadResult<Workload> {
        self.storage.get(workload_id)
    }

    pub(crate) async fn start(&self, workload_id: &str) -> WorkloadResult<Workload> {
        let workload = self.storage.get(workload_id)?;
        workload.kind.policy().ensure_start_allowed()?;
        if workload.desired_state == DesiredState::Deleted
            || workload.actual_state == ActualState::Deleted
            || !self
                .storage
                .set_desired_running_unless_deleted_and_clear_backoff(workload_id)?
        {
            return Err(WorkloadError::failed_precondition(
                "deleted workloads cannot be started",
            ));
        }

        self.storage.get(workload_id)
    }

    pub(crate) async fn stop(&self, workload_id: &str) -> WorkloadResult<Workload> {
        let workload = self.storage.get(workload_id)?;
        if workload.desired_state == DesiredState::Deleted
            || workload.actual_state == ActualState::Deleted
            || !self
                .storage
                .set_desired_unless_deleted(workload_id, DesiredState::Stopped)?
        {
            return Err(WorkloadError::failed_precondition(
                "deleted workloads cannot be stopped",
            ));
        }

        self.storage.get(workload_id)
    }

    pub(crate) async fn delete(&self, workload_id: &str) -> WorkloadResult<Workload> {
        self.storage.get(workload_id)?;
        self.storage
            .set_desired(workload_id, DesiredState::Deleted)?;
        self.storage.get(workload_id)
    }

    pub(crate) async fn get_logs(&self, workload_id: &str) -> WorkloadResult<RuntimeLogs> {
        logs::get_logs(self, workload_id).await
    }

    pub(crate) async fn watch_once(&self) -> WorkloadResult<()> {
        watch::run_once(self).await
    }

    pub(crate) async fn reconcile_once(&self) -> WorkloadResult<()> {
        reconcile::run_once(self).await
    }

    pub(crate) async fn run_watch_loop(self) {
        loop {
            if let Err(error) = self.watch_once().await {
                eprintln!("billow-agent: workload watcher failed: {error}");
            }
            tokio::time::sleep(Self::watch_interval()).await;
        }
    }

    pub(crate) async fn run_reconcile_loop(self) {
        loop {
            if let Err(error) = self.reconcile_once().await {
                eprintln!("billow-agent: workload reconciler failed: {error}");
            }
            tokio::time::sleep(Self::reconcile_interval()).await;
        }
    }
}

fn db_path() -> std::path::PathBuf {
    env_path_or_default(DB_PATH_ENV, DEFAULT_DB_PATH)
}

pub(crate) fn error_status(error: WorkloadError) -> tonic::Status {
    match error.code() {
        types::ErrorCode::InvalidArgument => tonic::Status::invalid_argument(error.to_string()),
        types::ErrorCode::NotFound => tonic::Status::not_found(error.to_string()),
        types::ErrorCode::FailedPrecondition => {
            tonic::Status::failed_precondition(error.to_string())
        }
        types::ErrorCode::Internal => tonic::Status::internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::runtime::{
        ContainerRuntime, RuntimeCleanupMode, RuntimeLogSource, RuntimeLogs, RuntimeStartRequest,
        RuntimeStartResult, RuntimeStopMode, RuntimeTaskState, RuntimeTaskStatus,
    };
    use super::storage::WorkloadStorage;
    use super::types::{
        ActualState, CleanupState, DesiredState, ErrorCode, WorkloadKind, duration_secs,
    };
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FakeTask {
        state: RuntimeTaskState,
        exit_code: Option<u32>,
        container_ip: Option<String>,
    }

    #[derive(Clone)]
    struct FakeLogs {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    }

    #[derive(Default)]
    struct FakeRuntime {
        tasks: Mutex<HashMap<String, FakeTask>>,
        logs: Mutex<HashMap<String, FakeLogs>>,
        starts: Mutex<Vec<String>>,
        stops: Mutex<Vec<(String, RuntimeStopMode)>>,
        releases: Mutex<Vec<String>>,
        prunes: Mutex<Vec<(String, RuntimeCleanupMode)>>,
        ignore_graceful_stops: Mutex<bool>,
        ignore_force_stops: Mutex<bool>,
        start_failures_remaining: Mutex<usize>,
        release_failures_remaining: Mutex<usize>,
        prune_failures_remaining: Mutex<usize>,
    }

    impl FakeRuntime {
        fn ignore_graceful_stops(&self) {
            *self.ignore_graceful_stops.lock().unwrap() = true;
        }

        fn ignore_all_stops(&self) {
            *self.ignore_graceful_stops.lock().unwrap() = true;
            *self.ignore_force_stops.lock().unwrap() = true;
        }

        fn fail_next_starts(&self, count: usize) {
            *self.start_failures_remaining.lock().unwrap() = count;
        }

        fn fail_next_releases(&self, count: usize) {
            *self.release_failures_remaining.lock().unwrap() = count;
        }

        fn fail_next_prunes(&self, count: usize) {
            *self.prune_failures_remaining.lock().unwrap() = count;
        }

        fn insert_running_task(&self, runtime_task_id: &str) {
            self.tasks.lock().unwrap().insert(
                runtime_task_id.to_string(),
                FakeTask {
                    state: RuntimeTaskState::Running,
                    exit_code: None,
                    container_ip: Some(String::from("10.1.1.2")),
                },
            );
            self.logs.lock().unwrap().insert(
                runtime_task_id.to_string(),
                FakeLogs {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
            );
        }

        fn insert_created_task(&self, runtime_task_id: &str) {
            self.tasks.lock().unwrap().insert(
                runtime_task_id.to_string(),
                FakeTask {
                    state: RuntimeTaskState::Created,
                    exit_code: None,
                    container_ip: Some(String::from("10.1.1.2")),
                },
            );
            self.logs.lock().unwrap().insert(
                runtime_task_id.to_string(),
                FakeLogs {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
            );
        }

        fn drop_task(&self, runtime_task_id: &str) {
            self.tasks.lock().unwrap().remove(runtime_task_id);
        }

        fn drop_all_tasks(&self) {
            self.tasks.lock().unwrap().clear();
        }

        fn set_stopped(&self, runtime_task_id: &str, exit_code: u32) {
            let mut tasks = self.tasks.lock().unwrap();
            let task = tasks.get_mut(runtime_task_id).unwrap();
            task.state = RuntimeTaskState::Stopped;
            task.exit_code = Some(exit_code);
        }

        fn set_stopped_if_present(&self, runtime_task_id: &str, exit_code: u32) {
            if let Some(task) = self.tasks.lock().unwrap().get_mut(runtime_task_id) {
                task.state = RuntimeTaskState::Stopped;
                task.exit_code = Some(exit_code);
            }
        }

        fn set_unknown(&self, runtime_task_id: &str) {
            let mut tasks = self.tasks.lock().unwrap();
            let task = tasks.get_mut(runtime_task_id).unwrap();
            task.state = RuntimeTaskState::Unknown;
            task.exit_code = None;
        }

        fn task_exists(&self, runtime_task_id: &str) -> bool {
            self.tasks.lock().unwrap().contains_key(runtime_task_id)
        }

        fn start_count(&self) -> usize {
            self.starts.lock().unwrap().len()
        }

        fn stop_count(&self) -> usize {
            self.stops.lock().unwrap().len()
        }

        fn force_stop_count(&self) -> usize {
            self.stops
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, mode)| *mode == RuntimeStopMode::Force)
                .count()
        }

        fn prune_count(&self) -> usize {
            self.prunes.lock().unwrap().len()
        }

        fn remove_log_prune_count(&self) -> usize {
            self.prunes
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, mode)| *mode == RuntimeCleanupMode::RemoveLogs)
                .count()
        }

        fn preserve_log_prune_count(&self) -> usize {
            self.prunes
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, mode)| *mode == RuntimeCleanupMode::PreserveLogs)
                .count()
        }
    }

    #[tonic::async_trait]
    impl ContainerRuntime for FakeRuntime {
        fn log_source(&self, runtime_task_id: &str) -> RuntimeLogSource {
            RuntimeLogSource {
                runtime_task_id: runtime_task_id.to_string(),
            }
        }

        async fn start(&self, request: RuntimeStartRequest) -> WorkloadResult<RuntimeStartResult> {
            self.starts
                .lock()
                .unwrap()
                .push(request.runtime_task_id.clone());

            let (stdout, stderr) = match request.image.as_str() {
                "hello-world" => (b"Hello from Docker!\n".to_vec(), Vec::new()),
                "nginx" => (Vec::new(), b"start worker process\n".to_vec()),
                _ => (Vec::new(), Vec::new()),
            };
            self.logs
                .lock()
                .unwrap()
                .insert(request.runtime_task_id.clone(), FakeLogs { stdout, stderr });

            {
                let mut failures_remaining = self.start_failures_remaining.lock().unwrap();
                if *failures_remaining > 0 {
                    *failures_remaining -= 1;
                    return Err(WorkloadError::internal("fake runtime start failed"));
                }
            }

            self.tasks.lock().unwrap().insert(
                request.runtime_task_id.clone(),
                FakeTask {
                    state: RuntimeTaskState::Running,
                    exit_code: None,
                    container_ip: Some(String::from("10.1.1.2")),
                },
            );
            Ok(RuntimeStartResult {
                container_ip: Some(String::from("10.1.1.2")),
            })
        }

        async fn inspect(
            &self,
            runtime_task_id: &str,
        ) -> WorkloadResult<Option<RuntimeTaskStatus>> {
            Ok(self
                .tasks
                .lock()
                .unwrap()
                .get(runtime_task_id)
                .map(|task| RuntimeTaskStatus {
                    state: task.state,
                    exit_code: task.exit_code,
                }))
        }

        async fn stop(&self, runtime_task_id: &str, mode: RuntimeStopMode) -> WorkloadResult<()> {
            self.stops
                .lock()
                .unwrap()
                .push((runtime_task_id.to_string(), mode));
            let ignore_graceful = *self.ignore_graceful_stops.lock().unwrap();
            let ignore_force = *self.ignore_force_stops.lock().unwrap();
            let ignored = match mode {
                RuntimeStopMode::Graceful => ignore_graceful,
                RuntimeStopMode::Force => ignore_force,
            };
            if !ignored {
                self.set_stopped_if_present(runtime_task_id, 0);
            }
            Ok(())
        }

        async fn release_container(&self, runtime_task_id: &str) -> WorkloadResult<()> {
            self.releases
                .lock()
                .unwrap()
                .push(runtime_task_id.to_string());
            let mut failures_remaining = self.release_failures_remaining.lock().unwrap();
            if *failures_remaining > 0 {
                *failures_remaining -= 1;
                return Err(WorkloadError::internal("fake runtime release failed"));
            }
            drop(failures_remaining);

            self.tasks.lock().unwrap().remove(runtime_task_id);
            Ok(())
        }

        async fn prune_run(
            &self,
            runtime_task_id: &str,
            mode: RuntimeCleanupMode,
        ) -> WorkloadResult<()> {
            self.prunes
                .lock()
                .unwrap()
                .push((runtime_task_id.to_string(), mode));
            let mut failures_remaining = self.prune_failures_remaining.lock().unwrap();
            if *failures_remaining > 0 {
                *failures_remaining -= 1;
                return Err(WorkloadError::internal("fake runtime prune failed"));
            }
            drop(failures_remaining);

            if mode == RuntimeCleanupMode::RemoveLogs {
                self.logs.lock().unwrap().remove(runtime_task_id);
            }
            Ok(())
        }

        async fn read_logs(
            &self,
            source: RuntimeLogSource,
            limit_bytes: usize,
        ) -> WorkloadResult<RuntimeLogs> {
            let logs = self.logs.lock().unwrap();
            let Some(logs) = logs.get(&source.runtime_task_id) else {
                return Ok(RuntimeLogs {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    stdout_truncated: false,
                    stderr_truncated: false,
                });
            };
            let (stdout, stdout_truncated) = bounded(logs.stdout.clone(), limit_bytes);
            let (stderr, stderr_truncated) = bounded(logs.stderr.clone(), limit_bytes);
            Ok(RuntimeLogs {
                stdout,
                stderr,
                stdout_truncated,
                stderr_truncated,
            })
        }

        async fn container_ip(&self, runtime_task_id: &str) -> WorkloadResult<Option<String>> {
            Ok(self
                .tasks
                .lock()
                .unwrap()
                .get(runtime_task_id)
                .and_then(|task| task.container_ip.clone()))
        }
    }

    fn test_manager() -> (WorkloadManager, Arc<FakeRuntime>) {
        let runtime = Arc::new(FakeRuntime::default());
        let storage = WorkloadStorage::open_in_memory().unwrap();
        (
            WorkloadManager::new(storage, runtime.clone()).with_log_limit(1024),
            runtime,
        )
    }

    async fn submit_and_reach_running(
        manager: &WorkloadManager,
        kind: WorkloadKind,
        image: &str,
    ) -> Workload {
        let workload = manager.submit(kind, String::from(image)).await.unwrap();
        manager.reconcile_once().await.unwrap();
        let starting = manager.get(&workload.id).unwrap();
        assert_eq!(starting.actual_state, ActualState::Starting);
        manager.watch_once().await.unwrap();
        let running = manager.get(&workload.id).unwrap();
        assert_eq!(running.actual_state, ActualState::Running);
        running
    }

    fn expire_restart_backoff(manager: &WorkloadManager, workload_id: &str) {
        let workload = manager.get(workload_id).unwrap();
        manager
            .storage
            .set_restart_backoff_for_test(
                workload_id,
                workload.restart_attempts,
                Some(now_unix_secs().saturating_sub(1)),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn once_workload_runs_to_stopped_and_exposes_logs() {
        let (manager, runtime) = test_manager();

        let running = submit_and_reach_running(&manager, WorkloadKind::Once, "hello-world").await;
        runtime.set_stopped(running.runtime_task_id.as_deref().unwrap(), 0);
        manager.watch_once().await.unwrap();

        let stopped = manager.get(&running.id).unwrap();
        assert_eq!(stopped.actual_state, ActualState::Stopped);
        assert_eq!(stopped.exit_code, Some(0));
        let logs = manager.get_logs(&running.id).await.unwrap();
        assert_eq!(logs.stdout, b"Hello from Docker!\n");
    }

    #[tokio::test]
    async fn once_nonzero_exit_becomes_failed() {
        let (manager, runtime) = test_manager();

        let running = submit_and_reach_running(&manager, WorkloadKind::Once, "hello-world").await;
        runtime.set_stopped(running.runtime_task_id.as_deref().unwrap(), 42);
        manager.watch_once().await.unwrap();

        let failed = manager.get(&running.id).unwrap();
        assert_eq!(failed.actual_state, ActualState::Failed);
        assert_eq!(failed.exit_code, Some(42));
    }

    #[tokio::test]
    async fn service_submit_starts_immediately_when_reconciled() {
        let (manager, runtime) = test_manager();

        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        assert_eq!(running.actual_state, ActualState::Running);
        assert_eq!(running.container_ip.as_deref(), Some("10.1.1.2"));
        assert_eq!(runtime.start_count(), 1);
    }

    #[tokio::test]
    async fn cleanup_clears_latest_container_ip() {
        let (manager, _runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;

        manager.stop(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();
        manager.reconcile_once().await.unwrap();

        let stopped = manager.get(&running.id).unwrap();
        assert_eq!(stopped.actual_state, ActualState::Stopped);
        assert_eq!(stopped.container_ip, None);
    }

    #[tokio::test]
    async fn service_stop_reaches_stopped() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;

        manager.stop(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();

        let stopped = manager.get(&running.id).unwrap();
        assert_eq!(stopped.actual_state, ActualState::Stopped);
        assert_eq!(runtime.stop_count(), 1);
        assert_eq!(runtime.force_stop_count(), 0);
    }

    #[tokio::test]
    async fn explicit_stop_clears_nonzero_exit_code() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        let task_id = running.runtime_task_id.clone().unwrap();

        manager.stop(&running.id).await.unwrap();
        runtime.set_stopped(&task_id, 137);
        manager.watch_once().await.unwrap();

        let stopped = manager.get(&running.id).unwrap();
        assert_eq!(stopped.actual_state, ActualState::Stopped);
        assert_eq!(stopped.exit_code, None);
        assert!(stopped.error.is_none());
    }

    #[tokio::test]
    async fn desired_running_restarts_failed_service() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        let first_task_id = running.runtime_task_id.clone().unwrap();

        runtime.set_stopped(&first_task_id, 1);
        manager.watch_once().await.unwrap();
        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Failed
        );

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&running.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Failed);
        assert_eq!(backing_off.restart_attempts, 1);
        assert!(backing_off.restart_not_before_unix_secs.is_some());
        assert_eq!(runtime.start_count(), 1);

        expire_restart_backoff(&manager, &running.id);
        manager.reconcile_once().await.unwrap();

        let restarted = manager.get(&running.id).unwrap();
        assert_eq!(restarted.actual_state, ActualState::Starting);
        manager.watch_once().await.unwrap();
        let restarted = manager.get(&running.id).unwrap();
        assert_eq!(restarted.actual_state, ActualState::Running);
        assert_ne!(restarted.runtime_task_id.unwrap(), first_task_id);
        assert_eq!(runtime.prune_count(), 2);
        assert_eq!(runtime.remove_log_prune_count(), 1);
        assert_eq!(runtime.start_count(), 2);
    }

    #[tokio::test]
    async fn stopping_service_escalates_when_graceful_stop_does_not_exit() {
        let (manager, runtime) = test_manager();
        runtime.ignore_graceful_stops();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;

        manager.stop(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Stopping
        );
        assert_eq!(runtime.stop_count(), 1);

        let grace_secs = duration_secs(WorkloadManager::stop_grace());
        manager
            .storage
            .set_stopping_since_for_test(&running.id, now_unix_secs().saturating_sub(grace_secs))
            .unwrap();
        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();

        let stopped = manager.get(&running.id).unwrap();
        assert_eq!(stopped.actual_state, ActualState::Stopped);
        assert_eq!(runtime.stop_count(), 2);
        assert_eq!(runtime.force_stop_count(), 1);
    }

    #[tokio::test]
    async fn delete_tombstones_workload_and_keeps_logs() {
        let (manager, _) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;

        manager.delete(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();
        manager.reconcile_once().await.unwrap();

        let deleted = manager.get(&running.id).unwrap();
        assert_eq!(deleted.actual_state, ActualState::Deleted);
        let logs = manager.get_logs(&running.id).await.unwrap();
        assert_eq!(logs.stderr, b"start worker process\n");
    }

    #[tokio::test]
    async fn once_success_cleanup_hiccup_does_not_fail_workload() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Once, "hello-world").await;
        runtime.set_stopped(running.runtime_task_id.as_deref().unwrap(), 0);
        manager.watch_once().await.unwrap();

        runtime.fail_next_prunes(1);
        manager.reconcile_once().await.unwrap();

        let stopped = manager.get(&running.id).unwrap();
        assert_eq!(stopped.actual_state, ActualState::Stopped);
        assert_eq!(stopped.exit_code, Some(0));
        let run = manager.storage.latest_run(&running.id).unwrap().unwrap();
        assert!(run.container_released);
        assert_eq!(run.cleanup_state, CleanupState::Pending);
        assert_eq!(run.cleanup_attempts, 1);
        assert!(run.last_cleanup_error.is_some());

        manager
            .storage
            .backdate_run_for_test(&run.runtime_task_id, now_unix_secs().saturating_sub(3600))
            .unwrap();
        manager.reconcile_once().await.unwrap();
        let run = manager.storage.latest_run(&running.id).unwrap().unwrap();
        assert_eq!(run.cleanup_state, CleanupState::Done);
        assert_eq!(runtime.prune_count(), 2);
    }

    #[tokio::test]
    async fn start_during_stopping_waits_for_fresh_restart() {
        let (manager, runtime) = test_manager();
        runtime.ignore_graceful_stops();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        let first_task_id = running.runtime_task_id.clone().unwrap();

        manager.stop(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        manager.start(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();

        let draining = manager.get(&running.id).unwrap();
        assert_eq!(draining.desired_state, DesiredState::Running);
        assert_eq!(draining.actual_state, ActualState::Stopping);
        assert_eq!(
            draining.runtime_task_id.as_deref(),
            Some(first_task_id.as_str())
        );

        runtime.set_stopped(&first_task_id, 0);
        manager.watch_once().await.unwrap();
        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Stopped
        );

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&running.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Stopped);
        assert_eq!(backing_off.restart_attempts, 1);

        expire_restart_backoff(&manager, &running.id);
        manager.reconcile_once().await.unwrap();
        let restarting = manager.get(&running.id).unwrap();
        assert_eq!(restarting.actual_state, ActualState::Starting);
        assert_ne!(
            restarting.runtime_task_id.as_deref(),
            Some(first_task_id.as_str())
        );
        manager.watch_once().await.unwrap();
        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Running
        );
    }

    #[tokio::test]
    async fn unkillable_service_stays_stopping_and_surfaces_error() {
        let (manager, runtime) = test_manager();
        runtime.ignore_all_stops();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        let task_id = running.runtime_task_id.clone().unwrap();

        manager.delete(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Stopping
        );

        let timeout_secs = duration_secs(WorkloadManager::stop_grace())
            .saturating_add(duration_secs(WorkloadManager::stop_kill()));
        manager
            .storage
            .set_stopping_since_for_test(&running.id, now_unix_secs().saturating_sub(timeout_secs))
            .unwrap();
        manager.reconcile_once().await.unwrap();

        let stuck = manager.get(&running.id).unwrap();
        assert_eq!(stuck.actual_state, ActualState::Stopping);
        assert!(runtime.task_exists(&task_id));
        assert!(runtime.force_stop_count() >= 1);
        assert!(
            stuck
                .error
                .as_deref()
                .unwrap()
                .contains("stop escalation stuck")
        );
    }

    #[tokio::test]
    async fn delete_blocks_tombstone_until_container_released() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        runtime.fail_next_releases(1);

        manager.delete(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Stopped
        );

        manager.reconcile_once().await.unwrap();
        let blocked = manager.get(&running.id).unwrap();
        assert_eq!(blocked.actual_state, ActualState::Stopped);
        let run = manager.storage.latest_run(&running.id).unwrap().unwrap();
        assert!(!run.container_released);
        assert!(run.last_release_error.is_some());

        manager
            .storage
            .backdate_run_for_test(&run.runtime_task_id, now_unix_secs().saturating_sub(3600))
            .unwrap();
        manager.reconcile_once().await.unwrap();

        let deleted = manager.get(&running.id).unwrap();
        assert_eq!(deleted.actual_state, ActualState::Deleted);
    }

    #[tokio::test]
    async fn failed_run_release_failure_blocks_restart_until_retry_succeeds() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        let leaked_task_id = running.runtime_task_id.clone().unwrap();

        runtime.fail_next_releases(1);
        runtime.set_stopped(&leaked_task_id, 1);
        manager.watch_once().await.unwrap();

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&running.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Failed);
        let run = manager.storage.latest_run(&running.id).unwrap().unwrap();
        assert!(!run.container_released);
        assert!(run.last_release_error.is_some());

        expire_restart_backoff(&manager, &running.id);
        manager.reconcile_once().await.unwrap();

        let blocked = manager.get(&running.id).unwrap();
        assert_eq!(blocked.actual_state, ActualState::Failed);
        assert_eq!(
            blocked.runtime_task_id.as_deref(),
            Some(leaked_task_id.as_str())
        );
        assert_eq!(manager.storage.count_runs_for_test(&running.id).unwrap(), 1);
        assert_eq!(runtime.start_count(), 1);
        assert!(runtime.task_exists(&leaked_task_id));

        manager
            .storage
            .backdate_run_for_test(&leaked_task_id, now_unix_secs().saturating_sub(3600))
            .unwrap();
        expire_restart_backoff(&manager, &running.id);
        manager.reconcile_once().await.unwrap();

        let restarted = manager.get(&running.id).unwrap();
        assert_eq!(restarted.actual_state, ActualState::Starting);
        assert_ne!(
            restarted.runtime_task_id.as_deref(),
            Some(leaked_task_id.as_str())
        );
        assert!(!runtime.task_exists(&leaked_task_id));
        assert_eq!(runtime.start_count(), 2);
    }

    #[tokio::test]
    async fn start_failure_marks_failed_and_next_reconcile_restarts() {
        let (manager, runtime) = test_manager();
        runtime.fail_next_starts(1);
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();

        manager.reconcile_once().await.unwrap();
        let failed = manager.get(&workload.id).unwrap();
        assert_eq!(failed.actual_state, ActualState::Failed);
        assert!(!runtime.task_exists(failed.runtime_task_id.as_deref().unwrap()));
        let failed_run = manager.storage.latest_run(&workload.id).unwrap().unwrap();
        assert_eq!(failed_run.cleanup_state, CleanupState::Done);

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&workload.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Failed);
        assert_eq!(backing_off.restart_attempts, 1);

        expire_restart_backoff(&manager, &workload.id);
        manager.reconcile_once().await.unwrap();
        let restarting = manager.get(&workload.id).unwrap();
        assert_eq!(restarting.actual_state, ActualState::Starting);
        manager.watch_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Running
        );
        assert_eq!(runtime.start_count(), 2);
    }

    #[tokio::test]
    async fn creating_without_runtime_task_recovers_to_retryable_state() {
        let (manager, runtime) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        let runtime_task_id = String::from("billow-crashed-before-start");
        assert!(
            manager
                .storage
                .begin_start(&workload.id, ActualState::Accepted, &runtime_task_id)
                .unwrap()
        );

        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Accepted
        );
        assert!(manager.storage.latest_run(&workload.id).unwrap().is_none());

        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Running
        );
        assert_eq!(runtime.start_count(), 1);
    }

    #[tokio::test]
    async fn creating_with_existing_runtime_task_is_adopted() {
        let (manager, runtime) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        let runtime_task_id = String::from("billow-existing-start");
        assert!(
            manager
                .storage
                .begin_start(&workload.id, ActualState::Accepted, &runtime_task_id)
                .unwrap()
        );
        runtime.insert_running_task(&runtime_task_id);

        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Starting
        );
        manager.watch_once().await.unwrap();
        let adopted = manager.get(&workload.id).unwrap();
        assert_eq!(adopted.actual_state, ActualState::Running);
        assert_eq!(
            adopted.runtime_task_id.as_deref(),
            Some(runtime_task_id.as_str())
        );
    }

    #[tokio::test]
    async fn stop_requested_before_watch_does_not_resurrect_running_task() {
        let (manager, runtime) = test_manager();
        runtime.ignore_graceful_stops();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        manager.reconcile_once().await.unwrap();
        let starting = manager.get(&workload.id).unwrap();
        assert_eq!(starting.actual_state, ActualState::Starting);
        let task_id = starting.runtime_task_id.clone().unwrap();

        manager.stop(&workload.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Stopping
        );

        runtime.set_stopped(&task_id, 0);
        manager.watch_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Stopped
        );
    }

    #[tokio::test]
    async fn start_and_stop_reject_delete_in_progress() {
        let (manager, _) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        manager.reconcile_once().await.unwrap();
        manager.delete(&workload.id).await.unwrap();

        let start_error = manager.start(&workload.id).await.unwrap_err();
        let stop_error = manager.stop(&workload.id).await.unwrap_err();

        assert_eq!(start_error.code(), ErrorCode::FailedPrecondition);
        assert_eq!(stop_error.code(), ErrorCode::FailedPrecondition);
        assert_eq!(
            manager.get(&workload.id).unwrap().desired_state,
            DesiredState::Deleted
        );
    }

    #[tokio::test]
    async fn start_once_is_rejected_but_stop_is_allowed() {
        let (manager, _) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Once, String::from("hello-world"))
            .await
            .unwrap();

        let start_error = manager.start(&workload.id).await.unwrap_err();
        let stopped = manager.stop(&workload.id).await.unwrap();

        assert_eq!(start_error.code(), ErrorCode::FailedPrecondition);
        assert_eq!(stopped.desired_state, DesiredState::Stopped);

        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Stopped
        );
    }

    #[test]
    fn missing_workload_returns_not_found() {
        let (manager, _) = test_manager();

        let error = manager.get("missing").unwrap_err();

        assert_eq!(error.code(), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn storage_survives_manager_restart() {
        let runtime = Arc::new(FakeRuntime::default());
        let path = std::env::temp_dir().join(format!(
            "billow-workload-test-{}.sqlite3",
            Uuid::new_v4().simple()
        ));
        let storage = WorkloadStorage::open(&path).unwrap();
        let manager = WorkloadManager::new(storage, runtime.clone());
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        drop(manager);

        let storage = WorkloadStorage::open(&path).unwrap();
        let restarted = WorkloadManager::new(storage, runtime);
        let loaded = restarted.get(&workload.id).unwrap();

        assert_eq!(loaded.id, workload.id);
        assert_eq!(loaded.kind, WorkloadKind::Service);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn set_desired_does_not_override_delete() {
        let (manager, _) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        manager.delete(&workload.id).await.unwrap();

        let applied = manager
            .storage
            .set_desired_unless_deleted(&workload.id, DesiredState::Running)
            .unwrap();

        assert!(!applied);
        assert_eq!(
            manager.get(&workload.id).unwrap().desired_state,
            DesiredState::Deleted
        );
    }

    #[tokio::test]
    async fn begin_start_requires_desired_running() {
        let (manager, _) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        manager.stop(&workload.id).await.unwrap();

        let runtime_task_id = String::from("billow-should-not-start");
        let started = manager
            .storage
            .begin_start(&workload.id, ActualState::Accepted, &runtime_task_id)
            .unwrap();

        assert!(!started);
        assert!(manager.storage.latest_run(&workload.id).unwrap().is_none());
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Accepted
        );
    }

    #[tokio::test]
    async fn set_stopped_outcome_uses_current_desired_state() {
        let (manager, _) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        manager.stop(&running.id).await.unwrap();

        let applied = manager
            .storage
            .set_stopped_outcome(&running.id, ActualState::Running, Some(1))
            .unwrap();

        assert!(applied);
        let stopped = manager.get(&running.id).unwrap();
        assert_eq!(stopped.actual_state, ActualState::Stopped);
        assert!(stopped.error.is_none());
    }

    #[tokio::test]
    async fn recover_creating_with_release_failure_keeps_orphan_run() {
        let (manager, runtime) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        let runtime_task_id = String::from("billow-orphan");
        assert!(
            manager
                .storage
                .begin_start(&workload.id, ActualState::Accepted, &runtime_task_id)
                .unwrap()
        );
        runtime.fail_next_releases(1);

        manager.reconcile_once().await.unwrap();

        let recovered = manager.get(&workload.id).unwrap();
        assert_eq!(recovered.actual_state, ActualState::Creating);
        let failed_run = manager.storage.latest_run(&workload.id).unwrap().unwrap();
        assert_eq!(failed_run.runtime_task_id, runtime_task_id);
        assert!(!failed_run.container_released);
        assert!(failed_run.last_release_error.is_some());

        manager
            .storage
            .backdate_run_for_test(&runtime_task_id, now_unix_secs().saturating_sub(3600))
            .unwrap();
        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Accepted
        );
        assert!(manager.storage.latest_run(&workload.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn unknown_runtime_state_leaves_workload_unchanged() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;

        runtime.set_unknown(running.runtime_task_id.as_deref().unwrap());
        manager.watch_once().await.unwrap();

        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Running
        );
        assert!(
            manager
                .get(&running.id)
                .unwrap()
                .runtime_unknown_since_unix_secs
                .is_some()
        );
    }

    #[tokio::test]
    async fn persistent_unknown_runtime_state_fails_and_restarts_service_after_backoff() {
        let (manager, runtime) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        manager.reconcile_once().await.unwrap();
        let starting = manager.get(&workload.id).unwrap();
        assert_eq!(starting.actual_state, ActualState::Starting);
        let first_task_id = starting.runtime_task_id.clone().unwrap();

        runtime.set_unknown(&first_task_id);
        manager.watch_once().await.unwrap();
        let unknown = manager.get(&workload.id).unwrap();
        assert_eq!(unknown.actual_state, ActualState::Starting);
        assert!(unknown.runtime_unknown_since_unix_secs.is_some());

        manager
            .storage
            .set_runtime_unknown_since_for_test(
                &workload.id,
                now_unix_secs()
                    .saturating_sub(duration_secs(WorkloadManager::runtime_unknown_grace())),
            )
            .unwrap();
        manager.watch_once().await.unwrap();

        let failed = manager.get(&workload.id).unwrap();
        assert_eq!(failed.actual_state, ActualState::Failed);
        assert!(
            failed
                .error
                .as_deref()
                .unwrap()
                .contains("runtime task state remained unknown")
        );
        assert!(failed.runtime_unknown_since_unix_secs.is_none());

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&workload.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Failed);
        assert_eq!(backing_off.restart_attempts, 1);
        assert_eq!(runtime.start_count(), 1);

        expire_restart_backoff(&manager, &workload.id);
        manager.reconcile_once().await.unwrap();
        let restarting = manager.get(&workload.id).unwrap();
        assert_eq!(restarting.actual_state, ActualState::Starting);
        assert_ne!(
            restarting.runtime_task_id.as_deref(),
            Some(first_task_id.as_str())
        );
    }

    #[tokio::test]
    async fn failed_service_does_not_restart_before_backoff_deadline() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        runtime.set_stopped(running.runtime_task_id.as_deref().unwrap(), 1);
        manager.watch_once().await.unwrap();

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&running.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Failed);
        assert_eq!(backing_off.restart_attempts, 1);
        assert!(backing_off.restart_not_before_unix_secs.is_some());

        manager.reconcile_once().await.unwrap();
        let still_failed = manager.get(&running.id).unwrap();
        assert_eq!(still_failed.actual_state, ActualState::Failed);
        assert_eq!(runtime.start_count(), 1);
    }

    #[tokio::test]
    async fn service_restart_backoff_exponentially_increases_and_caps() {
        let (manager, runtime) = test_manager();
        runtime.fail_next_starts(8);
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();

        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Failed
        );

        let expected_delays = [5, 10, 20, 40, 80, 160, 300, 300];
        for (index, expected_delay) in expected_delays.into_iter().enumerate() {
            let before = now_unix_secs();
            manager.reconcile_once().await.unwrap();
            let backing_off = manager.get(&workload.id).unwrap();
            assert_eq!(backing_off.actual_state, ActualState::Failed);
            assert_eq!(backing_off.restart_attempts, (index as i64) + 1);

            let not_before = backing_off.restart_not_before_unix_secs.unwrap();
            let scheduled_delay = not_before.saturating_sub(before);
            assert!(
                scheduled_delay >= expected_delay && scheduled_delay <= expected_delay + 1,
                "expected delay around {expected_delay}, got {scheduled_delay}"
            );

            if index + 1 < expected_delays.len() {
                expire_restart_backoff(&manager, &workload.id);
                manager.reconcile_once().await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn stable_running_service_resets_restart_backoff() {
        let (manager, _) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;

        manager
            .storage
            .set_restart_backoff_for_test(&running.id, 4, None)
            .unwrap();
        manager
            .storage
            .set_running_since_for_test(
                &running.id,
                now_unix_secs()
                    .saturating_sub(duration_secs(WorkloadManager::restart_backoff_reset())),
            )
            .unwrap();

        manager.reconcile_once().await.unwrap();
        let reset = manager.get(&running.id).unwrap();
        assert_eq!(reset.actual_state, ActualState::Running);
        assert_eq!(reset.restart_attempts, 0);
        assert!(reset.restart_not_before_unix_secs.is_none());
    }

    #[tokio::test]
    async fn manual_start_clears_restart_backoff() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        runtime.set_stopped(running.runtime_task_id.as_deref().unwrap(), 1);
        manager.watch_once().await.unwrap();
        manager
            .storage
            .set_restart_backoff_for_test(&running.id, 3, Some(now_unix_secs().saturating_add(300)))
            .unwrap();

        manager.start(&running.id).await.unwrap();

        let started = manager.get(&running.id).unwrap();
        assert_eq!(started.desired_state, DesiredState::Running);
        assert_eq!(started.restart_attempts, 0);
        assert!(started.restart_not_before_unix_secs.is_none());
    }

    #[tokio::test]
    async fn start_failure_preserves_logs_for_diagnosis() {
        let (manager, runtime) = test_manager();
        runtime.fail_next_starts(1);
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();

        manager.reconcile_once().await.unwrap();

        assert_eq!(
            manager.get(&workload.id).unwrap().actual_state,
            ActualState::Failed
        );
        assert_eq!(runtime.preserve_log_prune_count(), 1);
        assert_eq!(runtime.remove_log_prune_count(), 0);
        let logs = manager.get_logs(&workload.id).await.unwrap();
        assert_eq!(logs.stderr, b"start worker process\n");
    }

    #[tokio::test]
    async fn escalated_service_restarts_only_after_task_confirmed_dead() {
        let (manager, runtime) = test_manager();
        runtime.ignore_all_stops();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        let task_id = running.runtime_task_id.clone().unwrap();

        manager.stop(&running.id).await.unwrap();
        manager.reconcile_once().await.unwrap();
        manager.start(&running.id).await.unwrap();

        let timeout_secs = duration_secs(WorkloadManager::stop_grace())
            .saturating_add(duration_secs(WorkloadManager::stop_kill()));
        manager
            .storage
            .set_stopping_since_for_test(&running.id, now_unix_secs().saturating_sub(timeout_secs))
            .unwrap();
        manager.reconcile_once().await.unwrap();

        let stuck = manager.get(&running.id).unwrap();
        assert_eq!(stuck.actual_state, ActualState::Stopping);
        assert_eq!(stuck.desired_state, DesiredState::Running);
        assert!(runtime.task_exists(&task_id));
        assert_eq!(runtime.start_count(), 1);

        runtime.set_stopped(&task_id, 0);
        manager.reconcile_once().await.unwrap();
        assert_eq!(
            manager.get(&running.id).unwrap().actual_state,
            ActualState::Stopped
        );

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&running.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Stopped);
        assert_eq!(backing_off.restart_attempts, 1);

        expire_restart_backoff(&manager, &running.id);
        manager.reconcile_once().await.unwrap();
        let restarted = manager.get(&running.id).unwrap();
        assert_eq!(restarted.actual_state, ActualState::Starting);
        assert_ne!(restarted.runtime_task_id.as_deref(), Some(task_id.as_str()));
        assert_eq!(runtime.start_count(), 2);
    }

    #[tokio::test]
    async fn creating_with_created_but_unstarted_task_is_retried() {
        let (manager, runtime) = test_manager();
        let workload = manager
            .submit(WorkloadKind::Service, String::from("nginx"))
            .await
            .unwrap();
        let runtime_task_id = String::from("billow-created-not-started");
        assert!(
            manager
                .storage
                .begin_start(&workload.id, ActualState::Accepted, &runtime_task_id)
                .unwrap()
        );
        runtime.insert_created_task(&runtime_task_id);

        manager.reconcile_once().await.unwrap();

        let recovered = manager.get(&workload.id).unwrap();
        assert_eq!(recovered.actual_state, ActualState::Accepted);
        assert!(!runtime.task_exists(&runtime_task_id));
        assert!(manager.storage.latest_run(&workload.id).unwrap().is_none());

        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();
        let started = manager.get(&workload.id).unwrap();
        assert_eq!(started.actual_state, ActualState::Running);
        assert_ne!(
            started.runtime_task_id.as_deref(),
            Some(runtime_task_id.as_str())
        );
        assert_eq!(runtime.start_count(), 1);
    }

    #[tokio::test]
    async fn containerd_restart_failed_service_restarts() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Service, "nginx").await;
        let task_id = running.runtime_task_id.clone().unwrap();

        runtime.drop_all_tasks();
        manager.watch_once().await.unwrap();
        let failed = manager.get(&running.id).unwrap();
        assert_eq!(failed.actual_state, ActualState::Failed);
        assert_eq!(failed.error.as_deref(), Some("runtime task not found"));

        manager.reconcile_once().await.unwrap();
        let backing_off = manager.get(&running.id).unwrap();
        assert_eq!(backing_off.actual_state, ActualState::Failed);
        assert_eq!(backing_off.restart_attempts, 1);

        expire_restart_backoff(&manager, &running.id);
        manager.reconcile_once().await.unwrap();
        manager.watch_once().await.unwrap();
        let restarted = manager.get(&running.id).unwrap();
        assert_eq!(restarted.actual_state, ActualState::Running);
        assert_ne!(restarted.runtime_task_id.as_deref(), Some(task_id.as_str()));
    }

    #[tokio::test]
    async fn lost_task_once_job_marked_failed_with_clear_error() {
        let (manager, runtime) = test_manager();
        let running = submit_and_reach_running(&manager, WorkloadKind::Once, "hello-world").await;

        runtime.drop_task(running.runtime_task_id.as_deref().unwrap());
        manager.watch_once().await.unwrap();

        let failed = manager.get(&running.id).unwrap();
        assert_eq!(failed.actual_state, ActualState::Failed);
        assert_eq!(
            failed.error.as_deref(),
            Some("runtime task disappeared before its exit was observed")
        );
    }

    fn bounded(mut bytes: Vec<u8>, limit: usize) -> (Vec<u8>, bool) {
        let truncated = bytes.len() > limit;
        if truncated {
            bytes.truncate(limit);
        }
        (bytes, truncated)
    }
}
