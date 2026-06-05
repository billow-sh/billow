use super::spec::runtime_spec;
use super::{NAMESPACE, RUNTIME_NAME, RuntimeResult, SNAPSHOTTER};
use containerd_client::services::v1::container::Runtime;
use containerd_client::services::v1::{Container, CreateContainerRequest};
use containerd_client::{Client, with_namespace};
use oci_spec::image::ImageConfiguration;
use std::collections::HashMap;
use tonic::Request;

pub(super) async fn create_container(
    client: &Client,
    image: &str,
    task_id: &str,
    snapshot_key: &str,
    image_config: &ImageConfiguration,
    args: Vec<String>,
) -> RuntimeResult<()> {
    let mut containers = client.containers();
    let spec = runtime_spec(task_id, image_config.config().as_ref(), args)?;

    containers
        .create(with_namespace!(
            CreateContainerRequest {
                container: Some(Container {
                    id: task_id.to_string(),
                    labels: HashMap::from([("io.billow.task.id".to_string(), task_id.to_string())]),
                    image: image.to_string(),
                    runtime: Some(Runtime {
                        name: RUNTIME_NAME.to_string(),
                        options: None,
                    }),
                    spec: Some(spec),
                    snapshotter: SNAPSHOTTER.to_string(),
                    snapshot_key: snapshot_key.to_string(),
                    ..Default::default()
                }),
            },
            NAMESPACE
        ))
        .await?;

    Ok(())
}
