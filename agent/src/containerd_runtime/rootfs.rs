use super::{NAMESPACE, RuntimeResult, SNAPSHOTTER, runtime_error};
use containerd_client::services::v1::snapshots::PrepareSnapshotRequest;
use containerd_client::types::Mount;
use containerd_client::{Client, with_namespace};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tonic::Request;

const ROOTFS_MOUNT_READY_TIMEOUT: Duration = Duration::from_secs(10);
const ROOTFS_MOUNT_READY_INTERVAL: Duration = Duration::from_millis(50);

pub(super) async fn prepare_rootfs(
    client: &Client,
    snapshot_key: &str,
    diff_ids: &[String],
) -> RuntimeResult<Vec<containerd_client::types::Mount>> {
    let parent_snapshot = chain_id(diff_ids)?;
    prepare_snapshot(client, snapshot_key, &parent_snapshot).await
}

async fn prepare_snapshot(
    client: &Client,
    snapshot_key: &str,
    parent_snapshot: &str,
) -> RuntimeResult<Vec<containerd_client::types::Mount>> {
    let response = client
        .snapshots()
        .prepare(with_namespace!(
            PrepareSnapshotRequest {
                snapshotter: SNAPSHOTTER.to_string(),
                key: snapshot_key.to_string(),
                parent: parent_snapshot.to_string(),
                labels: HashMap::new(),
            },
            NAMESPACE
        ))
        .await?
        .into_inner();

    Ok(response.mounts)
}

fn chain_id(diff_ids: &[String]) -> RuntimeResult<String> {
    let mut ids = diff_ids.iter();
    let Some(first) = ids.next() else {
        return Ok(String::new());
    };

    if !first.starts_with("sha256:") {
        return Err(runtime_error(format!("unsupported diff id digest {first}")));
    }

    ids.try_fold(first.clone(), |parent, diff_id| {
        if !diff_id.starts_with("sha256:") {
            return Err(runtime_error(format!(
                "unsupported diff id digest {diff_id}"
            )));
        }

        let mut hasher = Sha256::new();
        hasher.update(parent.as_bytes());
        hasher.update(b" ");
        hasher.update(diff_id.as_bytes());
        Ok(format!("sha256:{}", hex_digest(&hasher.finalize())))
    })
}

pub(super) async fn wait_for_mount_sources(mounts: &[Mount]) -> RuntimeResult<()> {
    let deadline = Instant::now() + ROOTFS_MOUNT_READY_TIMEOUT;

    loop {
        let missing = missing_mount_sources(mounts);
        if missing.is_empty() {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(runtime_error(format!(
                "containerd snapshot mount paths were not ready: {}",
                missing
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        sleep(ROOTFS_MOUNT_READY_INTERVAL).await;
    }
}

fn missing_mount_sources(mounts: &[Mount]) -> Vec<PathBuf> {
    mounts
        .iter()
        .flat_map(overlay_mount_source_paths)
        .filter(|path| !path.exists())
        .collect()
}

fn overlay_mount_source_paths(mount: &Mount) -> Vec<PathBuf> {
    if mount.r#type != "overlay" {
        return Vec::new();
    }

    let mut paths = Vec::new();
    for option in mount.options.iter().flat_map(|option| option.split(',')) {
        let Some((name, value)) = option.split_once('=') else {
            continue;
        };

        match name {
            "lowerdir" => {
                paths.extend(
                    value
                        .split(':')
                        .filter(|path| !path.is_empty())
                        .map(PathBuf::from),
                );
            }
            "upperdir" | "workdir" if !value.is_empty() => paths.push(PathBuf::from(value)),
            _ => {}
        }
    }

    paths
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_containerd_chain_id() {
        assert_eq!(chain_id(&["sha256:aaa".to_string()]).unwrap(), "sha256:aaa");
        assert_eq!(
            chain_id(&["sha256:aaa".to_string(), "sha256:bbb".to_string()]).unwrap(),
            "sha256:56efb1d4f6c79b745d37d6eff87e3ed8dd2be28104e124ba73fd6e6c4892c792"
        );
    }

    #[test]
    fn extracts_overlay_mount_source_paths() {
        let mount = Mount {
            r#type: "overlay".to_string(),
            source: "overlay".to_string(),
            target: "rootfs".to_string(),
            options: vec![
                "workdir=/snapshots/2/work,upperdir=/snapshots/2/fs".to_string(),
                "lowerdir=/snapshots/1/fs:/snapshots/0/fs".to_string(),
            ],
        };

        assert_eq!(
            overlay_mount_source_paths(&mount),
            vec![
                PathBuf::from("/snapshots/2/work"),
                PathBuf::from("/snapshots/2/fs"),
                PathBuf::from("/snapshots/1/fs"),
                PathBuf::from("/snapshots/0/fs"),
            ]
        );
    }
}
