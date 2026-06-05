use super::{NAMESPACE, RuntimeResult};
use containerd_client::Client;
use containerd_client::services::v1::{CreateNamespaceRequest, GetNamespaceRequest, Namespace};
use std::collections::HashMap;
use tonic::Code;

pub(super) async fn ensure_namespace(client: &Client) -> RuntimeResult<()> {
    let mut namespaces = client.namespaces();
    match namespaces
        .get(GetNamespaceRequest {
            name: NAMESPACE.to_string(),
        })
        .await
    {
        Ok(_) => Ok(()),
        Err(status) if status.code() == Code::NotFound => {
            namespaces
                .create(CreateNamespaceRequest {
                    namespace: Some(Namespace {
                        name: NAMESPACE.to_string(),
                        labels: HashMap::new(),
                    }),
                })
                .await?;
            Ok(())
        }
        Err(status) => Err(status.into()),
    }
}
