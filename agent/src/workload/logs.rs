use super::runtime::RuntimeLogs;
use super::{WorkloadManager, types::WorkloadResult};

pub(crate) async fn get_logs(
    manager: &WorkloadManager,
    workload_id: &str,
) -> WorkloadResult<RuntimeLogs> {
    manager.storage.get(workload_id)?;

    if let Some(run) = manager.storage.latest_run(workload_id)? {
        let source = manager.runtime.log_source(&run.runtime_task_id);
        return manager
            .runtime
            .read_logs(source, manager.log_limit_bytes)
            .await;
    }

    Ok(RuntimeLogs {
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
    })
}
