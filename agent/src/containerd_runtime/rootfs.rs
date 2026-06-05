use super::{NAMESPACE, RuntimeResult, SNAPSHOTTER, runtime_error};
use containerd_client::services::v1::snapshots::PrepareSnapshotRequest;
use containerd_client::{Client, with_namespace};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tonic::Request;

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
}
