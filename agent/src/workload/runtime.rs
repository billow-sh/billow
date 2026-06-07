use super::types::WorkloadResult;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeStartRequest {
    pub(crate) workload_id: String,
    pub(crate) runtime_task_id: String,
    pub(crate) image: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeLogSource {
    pub(crate) runtime_task_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeLogs {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeStopMode {
    Graceful,
    Force,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeCleanupMode {
    PreserveLogs,
    RemoveLogs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeTaskState {
    Created,
    Running,
    Stopped,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeTaskStatus {
    pub(crate) state: RuntimeTaskState,
    pub(crate) exit_code: Option<u32>,
}

#[tonic::async_trait]
pub(crate) trait ContainerRuntime: Send + Sync {
    fn log_source(&self, runtime_task_id: &str) -> RuntimeLogSource;

    async fn start(&self, request: RuntimeStartRequest) -> WorkloadResult<()>;

    async fn inspect(&self, runtime_task_id: &str) -> WorkloadResult<Option<RuntimeTaskStatus>>;

    async fn stop(&self, runtime_task_id: &str, mode: RuntimeStopMode) -> WorkloadResult<()>;

    async fn cleanup(&self, runtime_task_id: &str, mode: RuntimeCleanupMode) -> WorkloadResult<()>;

    async fn read_logs(
        &self,
        source: RuntimeLogSource,
        limit_bytes: usize,
    ) -> WorkloadResult<RuntimeLogs>;
}
