use super::{NAMESPACE, RuntimeResult, SNAPSHOTTER, runtime_error};
use containerd_client::services::v1::{
    GetImageRequest, ReadContentRequest, TransferOptions, TransferRequest,
};
use containerd_client::types::Platform;
use containerd_client::types::transfer::{ImageStore, OciRegistry, UnpackConfiguration};
use containerd_client::{Client, with_namespace};
use oci_spec::image::{Config, ImageConfiguration, ImageIndex, ImageManifest};
use std::env;
use tonic::Request;

pub(super) async fn pull_image(client: &Client, image: &str) -> RuntimeResult<()> {
    let platform = Platform {
        os: "linux".to_string(),
        architecture: host_architecture().to_string(),
        variant: String::new(),
        os_version: String::new(),
    };

    client
        .transfer()
        .transfer(with_namespace!(
            TransferRequest {
                source: Some(containerd_client::to_any(&OciRegistry {
                    reference: image.to_string(),
                    resolver: Default::default(),
                })),
                destination: Some(containerd_client::to_any(&ImageStore {
                    name: image.to_string(),
                    platforms: vec![platform.clone()],
                    unpacks: vec![UnpackConfiguration {
                        platform: Some(platform),
                        snapshotter: SNAPSHOTTER.to_string(),
                    }],
                    ..Default::default()
                })),
                options: Some(TransferOptions::default()),
            },
            NAMESPACE
        ))
        .await?;

    Ok(())
}

pub(super) async fn load_image_config(
    client: &Client,
    image: &str,
) -> RuntimeResult<ImageConfiguration> {
    let image = client
        .images()
        .get(with_namespace!(
            GetImageRequest {
                name: image.to_string(),
            },
            NAMESPACE
        ))
        .await?
        .into_inner()
        .image
        .ok_or_else(|| runtime_error("containerd returned an empty image response"))?;

    let target = image
        .target
        .ok_or_else(|| runtime_error("containerd image does not include a target descriptor"))?;
    let manifest_digest =
        resolve_manifest_digest(client, &target.media_type, &target.digest).await?;
    let manifest_bytes = read_content(client, &manifest_digest).await?;
    let manifest = ImageManifest::from_reader(manifest_bytes.as_slice())?;
    let config_bytes =
        read_content(client, manifest.config().digest().to_string().as_str()).await?;

    Ok(ImageConfiguration::from_reader(config_bytes.as_slice())?)
}

pub(super) fn image_command(config: Option<&Config>) -> RuntimeResult<Vec<String>> {
    let entrypoint = config
        .and_then(|config| config.entrypoint().clone())
        .unwrap_or_default();
    let cmd = config
        .and_then(|config| config.cmd().clone())
        .unwrap_or_default();

    let args = if entrypoint.is_empty() {
        cmd
    } else {
        entrypoint.into_iter().chain(cmd).collect()
    };

    if args.is_empty() {
        return Err(runtime_error(
            "image does not define an entrypoint or command for v1 run",
        ));
    }

    Ok(args)
}

async fn resolve_manifest_digest(
    client: &Client,
    media_type: &str,
    digest: &str,
) -> RuntimeResult<String> {
    if is_manifest_media_type(media_type) {
        return Ok(digest.to_string());
    }

    let bytes = read_content(client, digest).await?;
    if is_index_media_type(media_type) {
        return select_manifest_from_index(bytes.as_slice());
    }

    ImageManifest::from_reader(bytes.as_slice())
        .map(|_| digest.to_string())
        .or_else(|_| select_manifest_from_index(bytes.as_slice()))
}

fn select_manifest_from_index(bytes: &[u8]) -> RuntimeResult<String> {
    let index = ImageIndex::from_reader(bytes)?;
    let arch = host_architecture();

    index
        .manifests()
        .iter()
        .find(|descriptor| {
            descriptor.platform().as_ref().is_some_and(|platform| {
                platform.os().to_string() == "linux" && platform.architecture().to_string() == arch
            })
        })
        .or_else(|| {
            index.manifests().iter().find(|descriptor| {
                descriptor
                    .platform()
                    .as_ref()
                    .is_some_and(|platform| platform.os().to_string() == "linux")
            })
        })
        .map(|descriptor| descriptor.digest().to_string())
        .ok_or_else(|| runtime_error(format!("image index has no linux/{arch} manifest")))
}

async fn read_content(client: &Client, digest: &str) -> RuntimeResult<Vec<u8>> {
    let mut stream = client
        .content()
        .read(with_namespace!(
            ReadContentRequest {
                digest: digest.to_string(),
                offset: 0,
                size: 0,
            },
            NAMESPACE
        ))
        .await?
        .into_inner();

    let mut bytes = Vec::new();
    while let Some(message) = stream.message().await? {
        bytes.extend_from_slice(&message.data);
    }

    Ok(bytes)
}

fn is_manifest_media_type(media_type: &str) -> bool {
    media_type == "application/vnd.oci.image.manifest.v1+json"
        || media_type == "application/vnd.docker.distribution.manifest.v2+json"
}

fn is_index_media_type(media_type: &str) -> bool {
    media_type == "application/vnd.oci.image.index.v1+json"
        || media_type == "application/vnd.docker.distribution.manifest.list.v2+json"
}

fn host_architecture() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => arch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_spec::image::ConfigBuilder;

    #[test]
    fn builds_command_from_entrypoint_and_cmd() {
        let config = ConfigBuilder::default()
            .entrypoint(vec!["/bin/app".to_string()])
            .cmd(vec!["--serve".to_string()])
            .build()
            .unwrap();

        assert_eq!(
            image_command(Some(&config)).unwrap(),
            vec!["/bin/app".to_string(), "--serve".to_string()]
        );
    }

    #[test]
    fn uses_cmd_when_entrypoint_is_empty() {
        let config = ConfigBuilder::default()
            .cmd(vec!["/hello".to_string()])
            .build()
            .unwrap();

        assert_eq!(image_command(Some(&config)).unwrap(), vec!["/hello"]);
    }

    #[test]
    fn rejects_images_without_default_process() {
        let config = ConfigBuilder::default().build().unwrap();

        assert!(image_command(Some(&config)).is_err());
    }
}
