use super::run_io::path_string;
use super::{OCI_SPEC_TYPE_URL, RUNC_OPTIONS_TYPE_URL, RuntimeResult};
use oci_spec::image::Config;
use oci_spec::runtime::{
    Capabilities, Capability, Linux, LinuxCapabilities, LinuxNamespaceBuilder, LinuxNamespaceType,
    Process, Root, Spec, get_default_namespaces,
};
use prost::Message;
use prost_types::Any;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) fn runtime_spec(
    task_id: &str,
    config: Option<&Config>,
    args: Vec<String>,
    network_namespace: Option<&Path>,
) -> RuntimeResult<Any> {
    let mut process = Process::default();
    process.set_args(Some(args));
    process.set_env(Some(image_env(config)));
    process.set_cwd(image_cwd(config).into());
    process.set_capabilities(Some(default_capabilities()));

    let mut root = Root::default();
    root.set_path(PathBuf::from("rootfs"));
    root.set_readonly(Some(false));

    let mut spec = Spec::default();
    *spec.process_mut() = Some(process);
    *spec.root_mut() = Some(root);
    *spec.hostname_mut() = Some(task_id.to_string());
    if let Some(network_namespace) = network_namespace {
        set_network_namespace(&mut spec, network_namespace)?;
    }

    Ok(Any {
        type_url: OCI_SPEC_TYPE_URL.to_string(),
        value: serde_json::to_vec(&spec)?,
    })
}

fn set_network_namespace(spec: &mut Spec, path: &Path) -> RuntimeResult<()> {
    let linux = spec.linux_mut().get_or_insert_with(Linux::default);
    let mut namespaces = linux
        .namespaces()
        .clone()
        .unwrap_or_else(get_default_namespaces);
    let mut found = false;

    for namespace in &mut namespaces {
        if namespace.typ() == LinuxNamespaceType::Network {
            namespace.set_path(Some(path.to_path_buf()));
            found = true;
            break;
        }
    }

    if !found {
        namespaces.push(
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Network)
                .path(path.to_path_buf())
                .build()?,
        );
    }

    linux.set_namespaces(Some(namespaces));
    Ok(())
}

pub(super) fn runc_options(crun_path: &Path) -> RuntimeResult<Any> {
    let options = RuncOptions {
        binary_name: path_string(crun_path),
        ..Default::default()
    };
    let mut value = Vec::new();
    options.encode(&mut value)?;

    Ok(Any {
        type_url: RUNC_OPTIONS_TYPE_URL.to_string(),
        value,
    })
}

fn image_env(config: Option<&Config>) -> Vec<String> {
    config
        .and_then(|config| config.env().clone())
        .filter(|env| !env.is_empty())
        .unwrap_or_else(|| {
            vec![
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
                "TERM=xterm".to_string(),
            ]
        })
}

fn image_cwd(config: Option<&Config>) -> String {
    config
        .and_then(|config| config.working_dir().clone())
        .filter(|cwd| !cwd.is_empty())
        .unwrap_or_else(|| "/".to_string())
}

fn default_capabilities() -> LinuxCapabilities {
    let capabilities: Capabilities = HashSet::from([
        Capability::AuditWrite,
        Capability::Chown,
        Capability::DacOverride,
        Capability::Fowner,
        Capability::Fsetid,
        Capability::Kill,
        Capability::Mknod,
        Capability::NetBindService,
        Capability::NetRaw,
        Capability::Setfcap,
        Capability::Setgid,
        Capability::Setpcap,
        Capability::Setuid,
        Capability::SysChroot,
    ]);

    let mut linux_capabilities = LinuxCapabilities::default();
    linux_capabilities.set_bounding(Some(capabilities.clone()));
    linux_capabilities.set_effective(Some(capabilities.clone()));
    linux_capabilities.set_inheritable(Some(capabilities.clone()));
    linux_capabilities.set_permitted(Some(capabilities.clone()));
    linux_capabilities.set_ambient(Some(HashSet::new()));
    linux_capabilities
}

#[derive(Clone, PartialEq, Message)]
struct RuncOptions {
    #[prost(bool, tag = "1")]
    no_pivot_root: bool,
    #[prost(bool, tag = "2")]
    no_new_keyring: bool,
    #[prost(string, tag = "3")]
    shim_cgroup: String,
    #[prost(uint32, tag = "4")]
    io_uid: u32,
    #[prost(uint32, tag = "5")]
    io_gid: u32,
    #[prost(string, tag = "6")]
    binary_name: String,
    #[prost(string, tag = "7")]
    root: String,
    #[prost(bool, tag = "9")]
    systemd_cgroup: bool,
    #[prost(string, tag = "10")]
    criu_image_path: String,
    #[prost(string, tag = "11")]
    criu_work_path: String,
    #[prost(string, tag = "12")]
    task_api_address: String,
    #[prost(uint32, tag = "13")]
    task_api_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_spec_uses_existing_network_namespace_path() {
        let netns_path = Path::new("/run/billow/netns/task-1");
        let spec = runtime_spec(
            "task-1",
            None,
            vec![String::from("/bin/sh")],
            Some(netns_path),
        )
        .unwrap();
        let spec: Spec = serde_json::from_slice(&spec.value).unwrap();
        let namespaces = spec
            .linux()
            .as_ref()
            .unwrap()
            .namespaces()
            .as_ref()
            .unwrap();
        let network = namespaces
            .iter()
            .find(|namespace| namespace.typ() == LinuxNamespaceType::Network)
            .unwrap();

        assert_eq!(network.path().as_deref(), Some(netns_path));
    }

    #[test]
    fn default_capabilities_do_not_use_ambient_set() {
        let capabilities = default_capabilities();

        assert!(
            capabilities
                .ambient()
                .as_ref()
                .is_some_and(|ambient| ambient.is_empty())
        );
    }
}
