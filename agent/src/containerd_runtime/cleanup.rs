use super::network::NetworkConfig;
use super::{NAMESPACE, RuntimeResult, SNAPSHOTTER, runtime_error};
use containerd_client::services::v1::snapshots::RemoveSnapshotRequest;
use containerd_client::services::v1::{DeleteContainerRequest, DeleteTaskRequest};
use containerd_client::{Client, with_namespace};
use std::fs;
use std::io;
use std::path::PathBuf;
use tonic::{Code, Request};

pub(super) struct RunCleanup {
    task_id: String,
    snapshot_key: String,
    run_dir: PathBuf,
    network: NetworkConfig,
    network_created: bool,
    task_created: bool,
    container_created: bool,
    snapshot_created: bool,
}

impl RunCleanup {
    pub(super) fn new(
        task_id: String,
        snapshot_key: String,
        run_dir: PathBuf,
        network: NetworkConfig,
    ) -> Self {
        Self {
            task_id,
            snapshot_key,
            run_dir,
            network,
            network_created: false,
            task_created: false,
            container_created: false,
            snapshot_created: false,
        }
    }

    pub(super) fn existing(
        task_id: String,
        snapshot_key: String,
        run_dir: PathBuf,
        network: NetworkConfig,
    ) -> Self {
        Self {
            task_id,
            snapshot_key,
            run_dir,
            network,
            network_created: true,
            task_created: true,
            container_created: true,
            snapshot_created: true,
        }
    }

    pub(super) fn mark_task_created(&mut self) {
        self.task_created = true;
    }

    pub(super) fn mark_container_created(&mut self) {
        self.container_created = true;
    }

    pub(super) fn mark_network_created(&mut self) {
        self.network_created = true;
    }

    pub(super) fn mark_snapshot_created(&mut self) {
        self.snapshot_created = true;
    }

    pub(super) async fn release(&self, client: &Client) -> RuntimeResult<()> {
        let mut errors = Vec::new();

        if self.task_created {
            let mut tasks = client.tasks();
            if let Err(error) = tasks
                .delete(with_namespace!(
                    DeleteTaskRequest {
                        container_id: self.task_id.clone(),
                    },
                    NAMESPACE
                ))
                .await
            {
                if error.code() != Code::NotFound {
                    errors.push(format!("delete task {} failed: {error}", self.task_id));
                }
            }
        }

        if self.container_created {
            let mut containers = client.containers();
            if let Err(error) = containers
                .delete(with_namespace!(
                    DeleteContainerRequest {
                        id: self.task_id.clone(),
                    },
                    NAMESPACE
                ))
                .await
            {
                if error.code() != Code::NotFound {
                    errors.push(format!("delete container {} failed: {error}", self.task_id));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(runtime_error(errors.join("; ")))
        }
    }

    pub(super) async fn prune(&self, client: &Client, remove_run_dir: bool) -> RuntimeResult<()> {
        let mut errors = Vec::new();

        if self.snapshot_created {
            let mut snapshots = client.snapshots();
            if let Err(error) = snapshots
                .remove(with_namespace!(
                    RemoveSnapshotRequest {
                        snapshotter: SNAPSHOTTER.to_string(),
                        key: self.snapshot_key.clone(),
                    },
                    NAMESPACE
                ))
                .await
            {
                if error.code() != Code::NotFound {
                    errors.push(format!(
                        "remove snapshot {} failed: {error}",
                        self.snapshot_key
                    ));
                }
            }
        }

        if self.network_created {
            let network = self.network.clone();
            let task_id = self.task_id.clone();
            match tokio::task::spawn_blocking(move || network.cleanup(&task_id)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    errors.push(format!(
                        "cleanup CNI network {} failed: {error}",
                        self.task_id
                    ));
                }
                Err(error) => {
                    errors.push(format!(
                        "cleanup CNI network {} task panicked: {error}",
                        self.task_id
                    ));
                }
            }
        }

        if remove_run_dir {
            if let Err(error) = fs::remove_dir_all(&self.run_dir) {
                if error.kind() != io::ErrorKind::NotFound {
                    errors.push(format!(
                        "remove task directory {} failed: {error}",
                        self.run_dir.display()
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(runtime_error(errors.join("; ")))
        }
    }
}
