mod cleanup;
mod container;
mod image;
mod namespace;
mod network;
mod reference;
mod rootfs;
mod run_io;
mod spec;

use crate::workload::runtime::{
    ContainerRuntime, RuntimeCleanupMode, RuntimeLogSource, RuntimeLogs, RuntimeStartRequest,
    RuntimeStartResult, RuntimeStopMode, RuntimeTaskState, RuntimeTaskStatus,
};
use crate::workload::types::{WorkloadError, WorkloadResult, env_path_or_default};
use cleanup::RunCleanup;
use container::create_container;
use containerd_client::services::v1::{CreateTaskRequest, GetRequest, KillRequest, StartRequest};
use containerd_client::types::v1::Status as ContainerdTaskStatus;
use containerd_client::{Client, with_namespace};
use image::{image_command, load_image_config, pull_image};
use namespace::ensure_namespace;
use network::NetworkConfig;
use reference::normalize_image_reference;
use rootfs::{prepare_rootfs, wait_for_mount_sources};
use run_io::{StdioPaths, create_stdio_files, create_task_dir, path_string, read_bounded};
use spec::runc_options;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;
use tonic::Code;
use tonic::Request;

const DEFAULT_CONTAINERD_SOCKET: &str = "/run/billow/containerd/containerd.sock";
const DEFAULT_CONTAINERD_SHIM: &str = "/usr/local/lib/billow/bin/containerd-shim-runc-v2";
const DEFAULT_CRUN: &str = "/usr/local/lib/billow/bin/crun";
const DEFAULT_TASK_DIR: &str = "/run/billow/tasks";
const CONTAINERD_SOCKET_ENV: &str = "BILLOW_CONTAINERD_SOCKET";
const CONTAINERD_SHIM_ENV: &str = "BILLOW_CONTAINERD_SHIM";
const CRUN_ENV: &str = "BILLOW_CRUN";
const TASK_DIR_ENV: &str = "BILLOW_TASK_DIR";

const NAMESPACE: &str = "billow";
const SNAPSHOTTER: &str = "overlayfs";
const RUNTIME_NAME: &str = "io.containerd.runc.v2";
const OCI_SPEC_TYPE_URL: &str = "types.containerd.io/opencontainers/runtime-spec/1/Spec";
const RUNC_OPTIONS_TYPE_URL: &str = "containerd.runc.v1.Options";

type RuntimeError = Box<dyn Error + Send + Sync>;
type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Clone)]
pub(crate) struct ContainerdRuntime {
    socket_path: PathBuf,
    shim_path: PathBuf,
    crun_path: PathBuf,
    task_dir: PathBuf,
    network: NetworkConfig,
    client: Arc<OnceCell<Client>>,
}

impl ContainerdRuntime {
    pub(crate) fn from_env() -> RuntimeResult<Self> {
        Ok(Self {
            socket_path: env_path_or_default(CONTAINERD_SOCKET_ENV, DEFAULT_CONTAINERD_SOCKET),
            shim_path: env_path_or_default(CONTAINERD_SHIM_ENV, DEFAULT_CONTAINERD_SHIM),
            crun_path: env_path_or_default(CRUN_ENV, DEFAULT_CRUN),
            task_dir: env_path_or_default(TASK_DIR_ENV, DEFAULT_TASK_DIR),
            network: NetworkConfig::from_env()?,
            client: Arc::new(OnceCell::new()),
        })
    }

    async fn start_inner(
        &self,
        client: &Client,
        image: &str,
        task_id: &str,
        snapshot_key: &str,
        run_dir: &std::path::Path,
        cleanup: &mut RunCleanup,
    ) -> RuntimeResult<RuntimeStartResult> {
        ensure_namespace(client).await?;
        pull_image(client, image).await?;

        let image_config = load_image_config(client, image).await?;
        let args = image_command(image_config.config().as_ref())?;
        let netns_path = self.network.netns_path(task_id);

        // The container is created before the snapshot is prepared so containerd's garbage
        // collector does not reap the snapshot before a container references it.
        create_container(
            client,
            image,
            task_id,
            snapshot_key,
            &image_config,
            args,
            Some(&netns_path),
        )
        .await?;
        cleanup.mark_container_created();

        let rootfs = prepare_rootfs(client, snapshot_key, image_config.rootfs().diff_ids()).await?;
        cleanup.mark_snapshot_created();
        wait_for_mount_sources(&rootfs).await?;

        let container_ip = {
            let network = self.network.clone();
            let task_id = task_id.to_string();
            let run_dir = run_dir.to_path_buf();
            tokio::task::spawn_blocking(move || network.setup(&task_id, &run_dir))
                .await
                .map_err(|error| runtime_error(format!("network setup task panicked: {error}")))??
        };
        cleanup.mark_network_created();

        let stdio = create_stdio_files(run_dir)?;
        let mut tasks = client.tasks();
        tasks
            .create(with_namespace!(
                CreateTaskRequest {
                    container_id: task_id.to_string(),
                    rootfs,
                    stdin: path_string(&stdio.stdin),
                    stdout: path_string(&stdio.stdout),
                    stderr: path_string(&stdio.stderr),
                    terminal: false,
                    options: Some(runc_options(&self.crun_path)?),
                    runtime_path: path_string(&self.shim_path),
                    ..Default::default()
                },
                NAMESPACE
            ))
            .await?;
        cleanup.mark_task_created();

        tasks
            .start(with_namespace!(
                StartRequest {
                    container_id: task_id.to_string(),
                    ..Default::default()
                },
                NAMESPACE
            ))
            .await?;

        Ok(RuntimeStartResult {
            container_ip: Some(container_ip),
        })
    }

    async fn connect(&self) -> RuntimeResult<&Client> {
        self.client
            .get_or_try_init(|| async {
                Client::from_path(&self.socket_path)
                    .await
                    .map_err(|error| -> RuntimeError {
                        io::Error::new(
                            io::ErrorKind::ConnectionRefused,
                            format!(
                                "failed to connect to containerd at {}: {error}",
                                self.socket_path.display()
                            ),
                        )
                        .into()
                    })
            })
            .await
    }

    fn snapshot_key(runtime_task_id: &str) -> String {
        format!("{runtime_task_id}-rootfs")
    }

    fn run_dir(&self, runtime_task_id: &str) -> PathBuf {
        self.task_dir.join(runtime_task_id)
    }
}

#[tonic::async_trait]
impl ContainerRuntime for ContainerdRuntime {
    fn log_source(&self, runtime_task_id: &str) -> RuntimeLogSource {
        RuntimeLogSource {
            runtime_task_id: runtime_task_id.to_string(),
        }
    }

    async fn start(&self, request: RuntimeStartRequest) -> WorkloadResult<RuntimeStartResult> {
        let image = normalize_image_reference(&request.image).map_err(workload_runtime_error)?;
        let snapshot_key = Self::snapshot_key(&request.runtime_task_id);
        let run_dir = self.run_dir(&request.runtime_task_id);

        create_task_dir(&run_dir).map_err(workload_runtime_error)?;

        let client = self.connect().await.map_err(workload_runtime_error)?;
        let mut cleanup = RunCleanup::new(
            request.runtime_task_id.clone(),
            snapshot_key.clone(),
            run_dir.clone(),
            self.network.clone(),
        );
        match self
            .start_inner(
                client,
                &image,
                &request.runtime_task_id,
                &snapshot_key,
                &run_dir,
                &mut cleanup,
            )
            .await
        {
            Ok(result) => Ok(result),
            Err(error) => {
                let cleanup_result = async {
                    cleanup.release(client).await?;
                    cleanup.prune(client, false).await
                }
                .await;
                match cleanup_result {
                    Ok(()) => Err(workload_runtime_error(error)),
                    Err(cleanup_error) => Err(workload_runtime_error(runtime_error(format!(
                        "{error}; cleanup also failed: {cleanup_error}"
                    )))),
                }
            }
        }
    }

    async fn inspect(&self, runtime_task_id: &str) -> WorkloadResult<Option<RuntimeTaskStatus>> {
        let client = self.connect().await.map_err(workload_runtime_error)?;
        let mut tasks = client.tasks();
        let response = tasks
            .get(with_namespace!(
                GetRequest {
                    container_id: runtime_task_id.to_string(),
                    ..Default::default()
                },
                NAMESPACE
            ))
            .await;

        let process = match response {
            Ok(response) => response.into_inner().process,
            Err(error) if error.code() == Code::NotFound => return Ok(None),
            Err(error) => return Err(WorkloadError::internal(error.to_string())),
        };
        let Some(process) = process else {
            return Ok(None);
        };

        let state = match ContainerdTaskStatus::try_from(process.status) {
            Ok(ContainerdTaskStatus::Created) => RuntimeTaskState::Created,
            Ok(ContainerdTaskStatus::Running) => RuntimeTaskState::Running,
            Ok(ContainerdTaskStatus::Stopped) => RuntimeTaskState::Stopped,
            // We never pause tasks; a task paused by external intervention is treated as
            // running and left alone rather than fought by the reconciler.
            Ok(ContainerdTaskStatus::Paused) | Ok(ContainerdTaskStatus::Pausing) => {
                RuntimeTaskState::Running
            }
            Ok(ContainerdTaskStatus::Unknown) => RuntimeTaskState::Unknown,
            Err(error) => {
                return Err(WorkloadError::internal(format!(
                    "containerd reported undecodable task status {} for {runtime_task_id}: {error}",
                    process.status
                )));
            }
        };

        Ok(Some(RuntimeTaskStatus {
            state,
            exit_code: (state == RuntimeTaskState::Stopped).then_some(process.exit_status),
        }))
    }

    async fn stop(&self, runtime_task_id: &str, mode: RuntimeStopMode) -> WorkloadResult<()> {
        let client = self.connect().await.map_err(workload_runtime_error)?;
        let mut tasks = client.tasks();
        let signal = match mode {
            RuntimeStopMode::Graceful => 15,
            RuntimeStopMode::Force => 9,
        };
        let result = tasks
            .kill(with_namespace!(
                KillRequest {
                    container_id: runtime_task_id.to_string(),
                    signal,
                    all: true,
                    ..Default::default()
                },
                NAMESPACE
            ))
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(error) if error.code() == Code::NotFound => Ok(()),
            Err(error) => Err(WorkloadError::internal(error.to_string())),
        }
    }

    async fn release_container(&self, runtime_task_id: &str) -> WorkloadResult<()> {
        let client = self.connect().await.map_err(workload_runtime_error)?;
        RunCleanup::existing(
            runtime_task_id.to_string(),
            Self::snapshot_key(runtime_task_id),
            self.run_dir(runtime_task_id),
            self.network.clone(),
        )
        .release(client)
        .await
        .map_err(workload_runtime_error)
    }

    async fn prune_run(
        &self,
        runtime_task_id: &str,
        mode: RuntimeCleanupMode,
    ) -> WorkloadResult<()> {
        let client = self.connect().await.map_err(workload_runtime_error)?;
        let remove_run_dir = mode == RuntimeCleanupMode::RemoveLogs;
        RunCleanup::existing(
            runtime_task_id.to_string(),
            Self::snapshot_key(runtime_task_id),
            self.run_dir(runtime_task_id),
            self.network.clone(),
        )
        .prune(client, remove_run_dir)
        .await
        .map_err(workload_runtime_error)
    }

    async fn read_logs(
        &self,
        source: RuntimeLogSource,
        limit_bytes: usize,
    ) -> WorkloadResult<RuntimeLogs> {
        let stdio = StdioPaths::for_run_dir(&self.run_dir(&source.runtime_task_id));
        tokio::task::spawn_blocking(move || {
            let (stdout, stdout_truncated) = read_bounded(&stdio.stdout, limit_bytes)?;
            let (stderr, stderr_truncated) = read_bounded(&stdio.stderr, limit_bytes)?;
            Ok::<RuntimeLogs, RuntimeError>(RuntimeLogs {
                stdout,
                stderr,
                stdout_truncated,
                stderr_truncated,
            })
        })
        .await
        .map_err(|error| workload_runtime_error(Box::new(error)))?
        .map_err(workload_runtime_error)
    }

    async fn container_ip(&self, runtime_task_id: &str) -> WorkloadResult<Option<String>> {
        self.network
            .container_ip(&self.run_dir(runtime_task_id))
            .map_err(workload_runtime_error)
    }
}

fn runtime_error(message: impl Into<String>) -> RuntimeError {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}

fn workload_runtime_error(error: RuntimeError) -> WorkloadError {
    WorkloadError::internal(error.to_string())
}
