mod cleanup;
mod container;
mod image;
mod namespace;
mod reference;
mod rootfs;
mod run_io;
mod spec;

use billow_api::api::RunResponse;
use cleanup::RunCleanup;
use container::create_container;
use containerd_client::services::v1::{CreateTaskRequest, StartRequest, WaitRequest};
use containerd_client::{Client, with_namespace};
use image::{image_command, load_image_config, pull_image};
use namespace::ensure_namespace;
use reference::normalize_image_reference;
use rootfs::prepare_rootfs;
use run_io::{create_stdio_files, create_task_dir, path_string, read_bounded};
use spec::runc_options;
use std::env;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use tonic::Request;
use uuid::Uuid;

const DEFAULT_CONTAINERD_SOCKET: &str = "/run/billow/containerd/containerd.sock";
const DEFAULT_CONTAINERD_SHIM: &str = "/usr/local/lib/billow/bin/containerd-shim-runc-v2";
const DEFAULT_CRUN: &str = "/usr/local/lib/billow/bin/crun";
const DEFAULT_TASK_DIR: &str = "/run/billow/tasks";
const DEFAULT_LOG_LIMIT_BYTES: usize = 1024 * 1024;

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

#[derive(Clone, Debug)]
pub(crate) struct ContainerdRuntime {
    socket_path: PathBuf,
    shim_path: PathBuf,
    crun_path: PathBuf,
    task_dir: PathBuf,
    log_limit_bytes: usize,
}

impl Default for ContainerdRuntime {
    fn default() -> Self {
        Self {
            socket_path: env_path(CONTAINERD_SOCKET_ENV, DEFAULT_CONTAINERD_SOCKET),
            shim_path: env_path(CONTAINERD_SHIM_ENV, DEFAULT_CONTAINERD_SHIM),
            crun_path: env_path(CRUN_ENV, DEFAULT_CRUN),
            task_dir: env_path(TASK_DIR_ENV, DEFAULT_TASK_DIR),
            log_limit_bytes: DEFAULT_LOG_LIMIT_BYTES,
        }
    }
}

impl ContainerdRuntime {
    pub(crate) async fn run(&self, image: &str) -> RuntimeResult<RunResponse> {
        let image = normalize_image_reference(image)?;
        let task_id = format!("billow-{}", Uuid::new_v4().simple());
        let snapshot_key = format!("{task_id}-rootfs");
        let run_dir = self.task_dir.join(&task_id);

        create_task_dir(&run_dir)?;

        let client = Client::from_path(&self.socket_path)
            .await
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!(
                        "failed to connect to containerd at {}: {error}",
                        self.socket_path.display()
                    ),
                )
            })?;

        let mut cleanup = RunCleanup::new(task_id.clone(), snapshot_key.clone(), run_dir.clone());
        let run_result = self
            .run_inner(
                &client,
                &image,
                &task_id,
                &snapshot_key,
                &run_dir,
                &mut cleanup,
            )
            .await;
        let cleanup_result = cleanup.cleanup(&client).await;

        match (run_result, cleanup_result) {
            (Ok(response), Ok(())) => Ok(response),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup_error)) => Err(runtime_error(format!(
                "{error}; cleanup also failed: {cleanup_error}"
            ))),
        }
    }

    async fn run_inner(
        &self,
        client: &Client,
        image: &str,
        task_id: &str,
        snapshot_key: &str,
        run_dir: &std::path::Path,
        cleanup: &mut RunCleanup,
    ) -> RuntimeResult<RunResponse> {
        ensure_namespace(client).await?;
        pull_image(client, image).await?;

        let image_config = load_image_config(client, image).await?;
        let args = image_command(image_config.config().as_ref())?;
        let rootfs = prepare_rootfs(client, snapshot_key, image_config.rootfs().diff_ids()).await?;
        cleanup.mark_snapshot_created();

        create_container(client, image, task_id, snapshot_key, &image_config, args).await?;
        cleanup.mark_container_created();

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

        let wait_response = tasks
            .wait(with_namespace!(
                WaitRequest {
                    container_id: task_id.to_string(),
                    ..Default::default()
                },
                NAMESPACE
            ))
            .await?
            .into_inner();

        let (stdout, stdout_truncated) = read_bounded(&stdio.stdout, self.log_limit_bytes)?;
        let (stderr, stderr_truncated) = read_bounded(&stdio.stderr, self.log_limit_bytes)?;

        Ok(RunResponse {
            task_id: task_id.to_string(),
            exit_code: wait_response.exit_status,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
        })
    }
}

fn env_path(env_name: &str, default: &str) -> PathBuf {
    env::var_os(env_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn runtime_error(message: impl Into<String>) -> RuntimeError {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}
