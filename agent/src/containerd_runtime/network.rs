use super::{RuntimeResult, runtime_error};
use crate::workload::types::{env_path_or_default, env_string_or_default};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const DEFAULT_CNI_PLUGIN_DIR: &str = "/usr/local/lib/billow/bin/cni";
const DEFAULT_CNI_NETNS_DIR: &str = "/run/billow/netns";
const DEFAULT_CNI_IPAM_DIR: &str = "/var/lib/billow/cni/ipam";
const DEFAULT_CNI_NETWORK_NAME: &str = "billow-net";
const DEFAULT_CNI_BRIDGE_NAME: &str = "billow0";
const DEFAULT_CNI_SUBNET: &str = "10.1.1.0/24";
const CNI_PLUGIN_DIR_ENV: &str = "BILLOW_CNI_PLUGIN_DIR";
const CNI_NETNS_DIR_ENV: &str = "BILLOW_CNI_NETNS_DIR";
const CNI_IPAM_DIR_ENV: &str = "BILLOW_CNI_IPAM_DIR";
const CNI_NETWORK_NAME_ENV: &str = "BILLOW_CNI_NETWORK_NAME";
const CNI_BRIDGE_NAME_ENV: &str = "BILLOW_CNI_BRIDGE_NAME";
const CNI_SUBNET_ENV: &str = "BILLOW_CNI_SUBNET";
const CNI_VERSION: &str = "1.0.0";
const CONTAINER_IFNAME: &str = "eth0";
const LOOPBACK_IFNAME: &str = "lo";
const NETWORK_METADATA_FILE: &str = "network.json";

#[derive(Clone, Debug)]
pub(super) struct NetworkConfig {
    plugin_dir: PathBuf,
    netns_dir: PathBuf,
    ipam_dir: PathBuf,
    network_name: String,
    bridge_name: String,
    subnet: String,
    range: Ipv4Range,
}

impl NetworkConfig {
    pub(super) fn from_env() -> RuntimeResult<Self> {
        let subnet = env_string_or_default(CNI_SUBNET_ENV, DEFAULT_CNI_SUBNET);
        let range = Ipv4Range::from_subnet(&subnet)?;
        Ok(Self {
            plugin_dir: env_path_or_default(CNI_PLUGIN_DIR_ENV, DEFAULT_CNI_PLUGIN_DIR),
            netns_dir: env_path_or_default(CNI_NETNS_DIR_ENV, DEFAULT_CNI_NETNS_DIR),
            ipam_dir: env_path_or_default(CNI_IPAM_DIR_ENV, DEFAULT_CNI_IPAM_DIR),
            network_name: env_string_or_default(CNI_NETWORK_NAME_ENV, DEFAULT_CNI_NETWORK_NAME),
            bridge_name: env_string_or_default(CNI_BRIDGE_NAME_ENV, DEFAULT_CNI_BRIDGE_NAME),
            subnet,
            range,
        })
    }

    pub(super) fn netns_path(&self, runtime_task_id: &str) -> PathBuf {
        self.netns_dir.join(runtime_task_id)
    }

    pub(super) fn setup(&self, runtime_task_id: &str, run_dir: &Path) -> RuntimeResult<String> {
        fs::create_dir_all(&self.netns_dir).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to create CNI netns directory {}: {error}",
                    self.netns_dir.display()
                ),
            )
        })?;
        fs::create_dir_all(&self.ipam_dir).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to create CNI IPAM directory {}: {error}",
                    self.ipam_dir.display()
                ),
            )
        })?;

        let loopback_config = self.loopback_config()?;
        let bridge_config = self.bridge_config()?;

        let netns_path = self.netns_path(runtime_task_id);
        persist_network_namespace(&netns_path)?;

        let output = match (|| {
            self.run_cni(
                "loopback",
                "ADD",
                runtime_task_id,
                &netns_path,
                LOOPBACK_IFNAME,
                &loopback_config,
            )?;
            self.run_cni(
                "bridge",
                "ADD",
                runtime_task_id,
                &netns_path,
                CONTAINER_IFNAME,
                &bridge_config,
            )
        })() {
            Ok(output) => output,
            Err(error) => {
                let cleanup_error = self.cleanup_network(
                    runtime_task_id,
                    &netns_path,
                    &bridge_config,
                    &loopback_config,
                );
                return match cleanup_error {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(runtime_error(format!(
                        "{error}; CNI cleanup also failed: {cleanup_error}"
                    ))),
                };
            }
        };

        let container_ip = parse_container_ip(&output)?;
        write_metadata(run_dir, &container_ip)?;
        Ok(container_ip)
    }

    pub(super) fn cleanup(&self, runtime_task_id: &str) -> RuntimeResult<()> {
        let netns_path = self.netns_path(runtime_task_id);
        let loopback_config = self.loopback_config()?;
        let bridge_config = self.bridge_config()?;

        self.cleanup_network(
            runtime_task_id,
            &netns_path,
            &bridge_config,
            &loopback_config,
        )
    }

    pub(super) fn container_ip(&self, run_dir: &Path) -> RuntimeResult<Option<String>> {
        read_container_ip(run_dir)
    }

    fn cleanup_network(
        &self,
        runtime_task_id: &str,
        netns_path: &Path,
        bridge_config: &[u8],
        loopback_config: &[u8],
    ) -> RuntimeResult<()> {
        self.run_cni(
            "bridge",
            "DEL",
            runtime_task_id,
            netns_path,
            CONTAINER_IFNAME,
            bridge_config,
        )
        .map_err(|error| runtime_error(format!("CNI bridge DEL failed: {error}")))?;

        if let Err(error) = self.run_cni(
            "loopback",
            "DEL",
            runtime_task_id,
            netns_path,
            LOOPBACK_IFNAME,
            loopback_config,
        ) {
            eprintln!("billow-agent: CNI loopback DEL for {runtime_task_id} ignored: {error}");
        }

        remove_network_namespace(netns_path)
    }

    fn loopback_config(&self) -> RuntimeResult<Vec<u8>> {
        Ok(serde_json::to_vec(&json!({
            "cniVersion": CNI_VERSION,
            "name": "lo",
            "type": "loopback",
        }))?)
    }

    fn bridge_config(&self) -> RuntimeResult<Vec<u8>> {
        let range = self.range;
        let ipam_dir = self.ipam_dir.display().to_string();
        Ok(serde_json::to_vec(&json!({
            "cniVersion": CNI_VERSION,
            "name": self.network_name.as_str(),
            "type": "bridge",
            "bridge": self.bridge_name.as_str(),
            "isGateway": true,
            "ipMasq": true,
            "ipam": {
                "type": "host-local",
                "ranges": [[{
                    "subnet": self.subnet.as_str(),
                    "rangeStart": range.range_start.to_string(),
                    "rangeEnd": range.range_end.to_string(),
                    "gateway": range.gateway.to_string(),
                }]],
                "routes": [
                    { "dst": "0.0.0.0/0" }
                ],
                "dataDir": ipam_dir,
            },
        }))?)
    }

    fn run_cni(
        &self,
        plugin_name: &str,
        command: &str,
        runtime_task_id: &str,
        netns_path: &Path,
        ifname: &str,
        config: &[u8],
    ) -> RuntimeResult<Vec<u8>> {
        let plugin_path = self.plugin_dir.join(plugin_name);
        let mut child = Command::new(&plugin_path)
            .env("CNI_COMMAND", command)
            .env("CNI_CONTAINERID", runtime_task_id)
            .env("CNI_NETNS", netns_path)
            .env("CNI_IFNAME", ifname)
            .env("CNI_PATH", &self.plugin_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "failed to run CNI plugin {} for {command}: {error}",
                        plugin_path.display()
                    ),
                )
            })?;

        child
            .stdin
            .take()
            .ok_or_else(|| runtime_error("CNI plugin stdin was not available"))?
            .write_all(config)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(runtime_error(format!(
                "CNI plugin {plugin_name} {command} exited with {}; stdout: {}; stderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        Ok(output.stdout)
    }
}

pub(super) fn parse_container_ip(output: &[u8]) -> RuntimeResult<String> {
    let value: Value = serde_json::from_slice(output)?;
    let ips = value
        .get("ips")
        .and_then(Value::as_array)
        .ok_or_else(|| runtime_error("CNI result did not include ips"))?;

    for ip in ips {
        let Some(address) = ip.get("address").and_then(Value::as_str) else {
            continue;
        };
        let ip = address.split_once('/').map(|(ip, _)| ip).unwrap_or(address);
        if let Ok(IpAddr::V4(ip)) = ip.parse::<IpAddr>() {
            return Ok(ip.to_string());
        }
    }

    Err(runtime_error("CNI result did not include an IPv4 address"))
}

fn metadata_path(run_dir: &Path) -> PathBuf {
    run_dir.join(NETWORK_METADATA_FILE)
}

fn write_metadata(run_dir: &Path, container_ip: &str) -> RuntimeResult<()> {
    let metadata = json!({ "container_ip": container_ip });
    fs::write(metadata_path(run_dir), serde_json::to_vec(&metadata)?).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to write CNI metadata in {}: {error}",
                run_dir.display()
            ),
        )
    })?;
    Ok(())
}

fn read_container_ip(run_dir: &Path) -> RuntimeResult<Option<String>> {
    let bytes = match fs::read(metadata_path(run_dir)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let container_ip = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .get("container_ip")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
    Ok(container_ip)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ipv4Range {
    gateway: Ipv4Addr,
    range_start: Ipv4Addr,
    range_end: Ipv4Addr,
}

impl Ipv4Range {
    fn from_subnet(subnet: &str) -> RuntimeResult<Self> {
        let (network, prefix) = subnet
            .split_once('/')
            .ok_or_else(|| runtime_error(format!("invalid CNI subnet {subnet:?}")))?;
        let network = network.parse::<Ipv4Addr>().map_err(|error| {
            runtime_error(format!("invalid CNI subnet address {subnet:?}: {error}"))
        })?;
        let prefix = prefix.parse::<u32>().map_err(|error| {
            runtime_error(format!("invalid CNI subnet prefix {subnet:?}: {error}"))
        })?;
        if prefix > 30 {
            return Err(runtime_error(format!(
                "CNI subnet {subnet:?} must have at least two usable addresses"
            )));
        }

        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        let network = u32::from(network) & mask;
        let broadcast = network | !mask;

        Ok(Self {
            gateway: Ipv4Addr::from(network.saturating_add(1)),
            range_start: Ipv4Addr::from(network.saturating_add(2)),
            range_end: Ipv4Addr::from(broadcast.saturating_sub(1)),
        })
    }
}

#[cfg(target_os = "linux")]
fn persist_network_namespace(path: &Path) -> RuntimeResult<()> {
    use std::ffi::CString;
    use std::fs::OpenOptions;
    use std::os::unix::ffi::OsStrExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to create network namespace {}: {error}",
                    path.display()
                ),
            )
        })?;

    let source = CString::new("/proc/self/ns/net").expect("static path has no nul bytes");
    let target = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        runtime_error(format!(
            "network namespace path {} contains a nul byte",
            path.display()
        ))
    })?;

    let child = unsafe { libc::fork() };
    if child < 0 {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(path);
        return Err(io::Error::new(
            error.kind(),
            format!("failed to fork network namespace helper: {error}"),
        )
        .into());
    }

    if child == 0 {
        if unsafe { libc::unshare(libc::CLONE_NEWNET) } != 0 {
            unsafe { libc::_exit(2) };
        }
        if unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        } != 0
        {
            unsafe { libc::_exit(3) };
        }
        unsafe { libc::_exit(0) };
    }

    let mut status = 0;
    loop {
        let result = unsafe { libc::waitpid(child, &mut status, 0) };
        if result == child {
            break;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        let _ = fs::remove_file(path);
        return Err(io::Error::new(
            error.kind(),
            format!("failed to wait for network namespace helper: {error}"),
        )
        .into());
    }

    if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
        return Ok(());
    }

    let _ = fs::remove_file(path);
    let detail = if libc::WIFEXITED(status) {
        match libc::WEXITSTATUS(status) {
            2 => "unshare failed",
            3 => "bind mount failed",
            _ => "unknown helper failure",
        }
    } else {
        "helper did not exit cleanly"
    };
    Err(runtime_error(format!(
        "failed to create network namespace {}: {detail}",
        path.display()
    )))
}

#[cfg(not(target_os = "linux"))]
fn persist_network_namespace(_path: &Path) -> RuntimeResult<()> {
    Err(runtime_error("CNI networking is only supported on Linux"))
}

#[cfg(target_os = "linux")]
fn remove_network_namespace(path: &Path) -> RuntimeResult<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if !path.exists() {
        return Ok(());
    }

    let target = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        runtime_error(format!(
            "network namespace path {} contains a nul byte",
            path.display()
        ))
    })?;
    if unsafe { libc::umount(target.as_ptr()) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINVAL) {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "failed to unmount network namespace {}: {error}",
                    path.display()
                ),
            )
            .into());
        }
    }

    fs::remove_file(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to remove network namespace {}: {error}",
                path.display()
            ),
        )
    })?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn remove_network_namespace(_path: &Path) -> RuntimeResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipv4_address_from_cni_result() {
        let output = br#"{
            "cniVersion": "1.0.0",
            "ips": [
                { "version": "6", "address": "fd00::2/64" },
                { "version": "4", "address": "10.1.1.7/24" }
            ]
        }"#;

        assert_eq!(parse_container_ip(output).unwrap(), "10.1.1.7");
    }

    #[test]
    fn cni_bridge_config_uses_billow_network_defaults() {
        let config = NetworkConfig {
            plugin_dir: PathBuf::from("/plugins"),
            netns_dir: PathBuf::from("/netns"),
            ipam_dir: PathBuf::from("/ipam"),
            network_name: String::from("billow-net"),
            bridge_name: String::from("billow0"),
            subnet: String::from("10.1.1.0/24"),
            range: Ipv4Range::from_subnet("10.1.1.0/24").unwrap(),
        };
        let value: Value = serde_json::from_slice(&config.bridge_config().unwrap()).unwrap();

        assert_eq!(value["name"], "billow-net");
        assert_eq!(value["bridge"], "billow0");
        assert_eq!(value["ipam"]["ranges"][0][0]["subnet"], "10.1.1.0/24");
        assert_eq!(value["ipam"]["ranges"][0][0]["rangeStart"], "10.1.1.2");
        assert_eq!(value["ipam"]["ranges"][0][0]["rangeEnd"], "10.1.1.254");
        assert_eq!(value["ipam"]["ranges"][0][0]["gateway"], "10.1.1.1");
        assert_eq!(value["ipam"]["dataDir"], "/ipam");
    }
}
