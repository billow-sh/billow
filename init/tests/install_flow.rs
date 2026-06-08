use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const INIT_BIN: &str = env!("CARGO_BIN_EXE_billow-init");
const TEST_BINARIES: [(&str, &str); 4] = [
    ("billow-agent", "fake agent\n"),
    ("containerd", "fake containerd\n"),
    ("containerd-shim-runc-v2", "fake shim\n"),
    ("crun", "fake crun\n"),
];
const TEST_CNI_PLUGINS: [(&str, &str); 3] = [
    ("bridge", "fake bridge\n"),
    ("host-local", "fake host-local\n"),
    ("loopback", "fake loopback\n"),
];

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    download_dir: PathBuf,
    install_bin_dir: PathBuf,
    config_dir: PathBuf,
    systemd_unit_dir: PathBuf,
    systemd_runtime_dir: PathBuf,
    fake_bin_dir: PathBuf,
    empty_path_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = unique_temp_dir();
        let download_dir = root.join("download");
        let install_bin_dir = root.join("usr-local-bin");
        let config_dir = root.join("etc-billow");
        let systemd_unit_dir = root.join("etc-systemd-system");
        let systemd_runtime_dir = root.join("run-systemd-system");
        let fake_bin_dir = root.join("fake-bin");
        let empty_path_dir = root.join("empty-path");
        let cni_download_dir = download_dir.join("cni");

        for dir in [
            &download_dir,
            &cni_download_dir,
            &install_bin_dir,
            &config_dir,
            &systemd_unit_dir,
            &systemd_runtime_dir,
            &fake_bin_dir,
            &empty_path_dir,
        ] {
            fs::create_dir_all(dir).expect("failed to create fixture directory");
        }

        fs::write(download_dir.join("billow-init"), "fake init\n")
            .expect("failed to create fake init");
        for (name, contents) in TEST_BINARIES {
            fs::write(download_dir.join(name), contents).expect("failed to create fake binary");
        }
        for (name, contents) in TEST_CNI_PLUGINS {
            fs::write(cni_download_dir.join(name), contents)
                .expect("failed to create fake CNI plugin");
        }
        write_fake_systemctl(&fake_bin_dir).expect("failed to create fake systemctl");

        Self {
            root,
            download_dir,
            install_bin_dir,
            config_dir,
            systemd_unit_dir,
            systemd_runtime_dir,
            fake_bin_dir,
            empty_path_dir,
        }
    }

    fn run(&self) -> Output {
        self.command().output().expect("failed to run billow-init")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(INIT_BIN);
        command
            .current_dir(&self.download_dir)
            .env("BILLOW_OVERRIDE_UID", "0")
            .env("BILLOW_DOWNLOAD_DIR", &self.download_dir)
            .env("BILLOW_BIN_DIR", &self.install_bin_dir)
            .env("BILLOW_CONFIG_DIR", &self.config_dir)
            .env("BILLOW_SYSTEMD_UNIT_DIR", &self.systemd_unit_dir)
            .env("BILLOW_SYSTEMD_RUNTIME_DIR", &self.systemd_runtime_dir)
            .env("PATH", self.fake_path());

        command
    }

    fn fake_path(&self) -> String {
        let existing_path = std::env::var_os("PATH").unwrap_or_default();

        format!(
            "{}:{}",
            self.fake_bin_dir.display(),
            existing_path.to_string_lossy()
        )
    }

    fn agent_source_path(&self) -> PathBuf {
        self.source_path("billow-agent")
    }

    fn agent_install_path(&self) -> PathBuf {
        self.install_path("billow-agent")
    }

    fn source_path(&self, name: &str) -> PathBuf {
        self.download_dir.join(name)
    }

    fn install_path(&self, name: &str) -> PathBuf {
        self.install_bin_dir.join(name)
    }

    fn cni_source_path(&self, name: &str) -> PathBuf {
        self.download_dir.join("cni").join(name)
    }

    fn cni_install_path(&self, name: &str) -> PathBuf {
        self.install_bin_dir.join("cni").join(name)
    }

    fn containerd_config_path(&self) -> PathBuf {
        self.config_dir.join("containerd").join("config.toml")
    }

    fn service_path(&self) -> PathBuf {
        self.systemd_unit_dir.join("billow-agent.service")
    }

    fn containerd_service_path(&self) -> PathBuf {
        self.systemd_unit_dir.join("billow-containerd.service")
    }

    fn systemctl_log_path(&self) -> PathBuf {
        self.fake_bin_dir.join("systemctl.log")
    }

    fn systemctl_log(&self) -> String {
        fs::read_to_string(self.systemctl_log_path()).unwrap_or_default()
    }

    fn fail_systemctl_command(&self, command: &str) {
        fs::write(self.fake_bin_dir.join(format!("fail-{command}")), "")
            .expect("failed to configure fake systemctl failure");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn happy_path_installs_agent_unit_and_starts_service() {
    let fixture = Fixture::new();

    let output = fixture.run();

    assert_success(&output);
    assert_all_sources_moved(&fixture);
    assert_all_binaries_installed(&fixture);

    let config =
        fs::read_to_string(fixture.containerd_config_path()).expect("failed to read config");
    assert!(config.contains("version = 3"));
    assert!(config.contains("[plugins.'io.containerd.cri.v1.runtime'.containerd]"));
    assert!(config.contains("default_runtime_name = 'runc'"));
    assert!(config.contains(&format!(
        "runtime_path = '{}'",
        fixture.install_path("containerd-shim-runc-v2").display()
    )));
    assert!(config.contains(&format!(
        "BinaryName = '{}'",
        fixture.install_path("crun").display()
    )));
    assert_mode(fixture.containerd_config_path(), 0o644);

    let unit = fs::read_to_string(fixture.service_path()).expect("failed to read service unit");
    assert!(unit.contains("Description=Billow Agent"));
    assert!(unit.contains("Requires=billow-containerd.service"));
    assert!(unit.contains("After=network.target billow-containerd.service"));
    assert!(unit.contains(&format!(
        "ExecStart={}",
        fixture.agent_install_path().display()
    )));
    assert!(
        unit.contains(
            "Environment=BILLOW_CONTAINERD_SOCKET=/run/billow/containerd/containerd.sock"
        )
    );
    assert!(unit.contains(&format!(
        "Environment=BILLOW_CONTAINERD_SHIM={}",
        fixture.install_path("containerd-shim-runc-v2").display()
    )));
    assert!(unit.contains(&format!(
        "Environment=BILLOW_CRUN={}",
        fixture.install_path("crun").display()
    )));
    assert!(unit.contains("Environment=BILLOW_TASK_DIR=/run/billow/tasks"));
    assert!(unit.contains("Environment=BILLOW_WORKLOAD_DB_PATH=/var/lib/billow/workloads.sqlite3"));
    assert!(unit.contains(&format!(
        "Environment=BILLOW_CNI_PLUGIN_DIR={}",
        fixture.install_bin_dir.join("cni").display()
    )));
    assert!(unit.contains("Environment=BILLOW_CNI_NETNS_DIR=/run/billow/netns"));
    assert!(unit.contains("Environment=BILLOW_CNI_IPAM_DIR=/var/lib/billow/cni/ipam"));
    assert!(unit.contains("Environment=BILLOW_CNI_NETWORK_NAME=billow-net"));
    assert!(unit.contains("Environment=BILLOW_CNI_BRIDGE_NAME=billow0"));
    assert!(unit.contains("Environment=BILLOW_CNI_SUBNET=10.1.1.0/24"));
    assert!(unit.contains("Restart=on-failure"));
    assert_mode(fixture.service_path(), 0o644);

    let containerd_unit = fs::read_to_string(fixture.containerd_service_path())
        .expect("failed to read containerd service unit");
    assert!(containerd_unit.contains("Description=Billow Containerd"));
    assert!(containerd_unit.contains("Type=notify"));
    assert!(containerd_unit.contains(&format!(
        "ExecStart={} --config {} --root /var/lib/billow/containerd --state /run/billow/containerd --address /run/billow/containerd/containerd.sock",
        fixture.install_path("containerd").display(),
        fixture.containerd_config_path().display()
    )));
    assert!(containerd_unit.contains(
        "ExecStartPre=/usr/bin/install -d -m 0755 /var/lib/billow/containerd /run/billow/containerd"
    ));
    assert!(containerd_unit.contains("Delegate=yes"));
    assert!(containerd_unit.contains("KillMode=process"));
    assert!(containerd_unit.contains("Restart=on-failure"));
    assert_mode(fixture.containerd_service_path(), 0o644);

    assert_eq!(
        fixture.systemctl_log(),
        "--version\ndaemon-reload\nenable --now billow-containerd.service billow-agent.service\n"
    );
    assert!(stdout(&output).contains(
        "billow-agent installed and started as billow-agent.service with billow-containerd.service"
    ));
}

#[test]
fn fails_without_root_rights() {
    let fixture = Fixture::new();

    let output = fixture
        .command()
        .env("BILLOW_OVERRIDE_UID", "1000")
        .output()
        .expect("failed to run billow-init");

    assert_failure_contains(&output, "must be run as root");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(fixture.systemctl_log(), "");
}

#[test]
fn fails_when_uid_override_is_invalid() {
    let fixture = Fixture::new();

    let output = fixture
        .command()
        .env("BILLOW_OVERRIDE_UID", "root")
        .output()
        .expect("failed to run billow-init");

    assert_failure_contains(&output, "BILLOW_OVERRIDE_UID must be an unsigned integer");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(fixture.systemctl_log(), "");
}

#[test]
fn fails_when_systemd_unit_dir_is_missing() {
    let fixture = Fixture::new();
    fs::remove_dir_all(&fixture.systemd_unit_dir).expect("failed to remove systemd unit dir");

    let output = fixture.run();

    assert_failure_contains(&output, "systemd unit directory");
    assert_failure_contains(&output, "does not exist");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(fixture.systemctl_log(), "");
}

#[test]
fn fails_when_systemctl_is_missing() {
    let fixture = Fixture::new();

    let output = fixture
        .command()
        .env("PATH", &fixture.empty_path_dir)
        .output()
        .expect("failed to run billow-init");

    assert_failure_contains(&output, "systemctl is not available");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(fixture.systemctl_log(), "");
}

#[test]
fn fails_when_systemctl_version_fails() {
    let fixture = Fixture::new();
    fixture.fail_systemctl_command("--version");

    let output = fixture.run();

    assert_failure_contains(&output, "systemctl is not available");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_systemd_runtime_dir_is_missing() {
    let fixture = Fixture::new();
    fs::remove_dir_all(&fixture.systemd_runtime_dir).expect("failed to remove runtime dir");

    let output = fixture.run();

    assert_failure_contains(&output, "systemd runtime directory");
    assert_failure_contains(&output, "does not exist");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_agent_is_already_installed() {
    let fixture = Fixture::new();
    fs::write(fixture.agent_install_path(), "installed already\n")
        .expect("failed to create installed agent");

    let output = fixture.run();

    assert_failure_contains(&output, "billow-agent already exists");
    assert_eq!(
        fs::read_to_string(fixture.agent_install_path()).expect("failed to read installed agent"),
        "installed already\n"
    );
    assert_all_sources_exist(&fixture);
    assert!(!fixture.install_path("containerd").exists());
    assert!(!fixture.install_path("containerd-shim-runc-v2").exists());
    assert!(!fixture.install_path("crun").exists());
    assert!(!fixture.cni_install_path("bridge").exists());
    assert!(!fixture.service_path().exists());
    assert!(!fixture.containerd_service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_containerd_binary_is_already_installed() {
    let fixture = Fixture::new();
    fs::write(fixture.install_path("containerd"), "installed already\n")
        .expect("failed to create installed containerd");

    let output = fixture.run();

    assert_failure_contains(&output, "containerd already exists");
    assert_all_sources_exist(&fixture);
    assert_eq!(
        fs::read_to_string(fixture.install_path("containerd"))
            .expect("failed to read installed containerd"),
        "installed already\n"
    );
    assert!(!fixture.service_path().exists());
    assert!(!fixture.containerd_service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_containerd_config_is_already_installed() {
    let fixture = Fixture::new();
    fs::create_dir_all(
        fixture
            .containerd_config_path()
            .parent()
            .expect("config path should have parent"),
    )
    .expect("failed to create containerd config directory");
    fs::write(fixture.containerd_config_path(), "installed already\n")
        .expect("failed to create installed containerd config");

    let output = fixture.run();

    assert_failure_contains(&output, "config.toml already exists");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(
        fs::read_to_string(fixture.containerd_config_path())
            .expect("failed to read containerd config"),
        "installed already\n"
    );
    assert!(!fixture.service_path().exists());
    assert!(!fixture.containerd_service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_agent_service_is_already_installed() {
    let fixture = Fixture::new();
    fs::write(fixture.service_path(), "installed already\n")
        .expect("failed to create installed unit");

    let output = fixture.run();

    assert_failure_contains(&output, "billow-agent.service already exists");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(
        fs::read_to_string(fixture.service_path()).expect("failed to read service unit"),
        "installed already\n"
    );
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_containerd_service_is_already_installed() {
    let fixture = Fixture::new();
    fs::write(fixture.containerd_service_path(), "installed already\n")
        .expect("failed to create installed containerd unit");

    let output = fixture.run();

    assert_failure_contains(&output, "billow-containerd.service already exists");
    assert_all_sources_exist(&fixture);
    assert_no_binaries_installed(&fixture);
    assert_eq!(
        fs::read_to_string(fixture.containerd_service_path())
            .expect("failed to read containerd service unit"),
        "installed already\n"
    );
    assert!(!fixture.service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_agent_source_is_missing() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.agent_source_path()).expect("failed to remove source agent");

    let output = fixture.run();

    assert_failure_contains(&output, "billow-agent must be present");
    assert_no_binaries_installed(&fixture);
    assert!(!fixture.service_path().exists());
    assert!(!fixture.containerd_service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_crun_source_is_missing() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.source_path("crun")).expect("failed to remove source crun");

    let output = fixture.run();

    assert_failure_contains(&output, "crun must be present");
    assert!(fixture.agent_source_path().exists());
    assert_no_binaries_installed(&fixture);
    assert!(!fixture.service_path().exists());
    assert!(!fixture.containerd_service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_cni_plugin_source_is_missing() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.cni_source_path("bridge")).expect("failed to remove source bridge");

    let output = fixture.run();

    assert_failure_contains(&output, "bridge must be present in the cni directory");
    assert_all_sources_exist_except(&fixture, "bridge");
    assert_no_binaries_installed(&fixture);
    assert!(!fixture.service_path().exists());
    assert!(!fixture.containerd_service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_daemon_reload_fails() {
    let fixture = Fixture::new();
    fixture.fail_systemctl_command("daemon-reload");

    let output = fixture.run();

    assert_failure_contains(&output, "systemctl daemon-reload failed");
    assert_all_sources_moved(&fixture);
    assert_all_binaries_installed(&fixture);
    assert!(fixture.containerd_config_path().exists());
    assert_mode(fixture.containerd_config_path(), 0o644);
    assert!(fixture.service_path().exists());
    assert!(fixture.containerd_service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\ndaemon-reload\n");
}

#[test]
fn fails_when_service_start_fails() {
    let fixture = Fixture::new();
    fixture.fail_systemctl_command("enable");

    let output = fixture.run();

    assert_failure_contains(
        &output,
        "systemctl enable --now billow-containerd.service billow-agent.service failed",
    );
    assert_all_sources_moved(&fixture);
    assert_all_binaries_installed(&fixture);
    assert!(fixture.containerd_config_path().exists());
    assert_mode(fixture.containerd_config_path(), 0o644);
    assert!(fixture.service_path().exists());
    assert!(fixture.containerd_service_path().exists());
    assert_eq!(
        fixture.systemctl_log(),
        "--version\ndaemon-reload\nenable --now billow-containerd.service billow-agent.service\n"
    );
}

fn unique_temp_dir() -> PathBuf {
    let fixture_id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();

    std::env::temp_dir().join(format!(
        "billow-init-test-{}-{timestamp}-{fixture_id}",
        std::process::id()
    ))
}

fn write_fake_systemctl(fake_bin_dir: &Path) -> io::Result<()> {
    let systemctl_path = fake_bin_dir.join("systemctl");

    fs::write(
        &systemctl_path,
        r#"#!/bin/bash
set -eu
script_dir="$(cd "$(dirname "$0")" && pwd)"
printf '%s\n' "$*" >> "$script_dir/systemctl.log"

if [ -f "$script_dir/fail-${1:-missing}" ]; then
  exit 1
fi

if [ "${1:-}" = "--version" ]; then
  printf 'systemd 255\n'
fi

exit 0
"#,
    )?;
    fs::set_permissions(systemctl_path, fs::Permissions::from_mode(0o755))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn assert_failure_contains(output: &Output, expected: &str) {
    assert!(
        !output.status.success(),
        "expected failure\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );

    let stderr = stderr(output);
    assert!(
        stderr.contains(expected),
        "expected stderr to contain {expected:?}\nstderr:\n{stderr}"
    );
}

fn assert_mode(path: impl AsRef<Path>, expected_mode: u32) {
    let mode = fs::metadata(path).expect("failed to stat path").mode() & 0o777;

    assert_eq!(mode, expected_mode);
}

fn assert_all_sources_exist(fixture: &Fixture) {
    for (name, _) in TEST_BINARIES {
        assert!(
            fixture.source_path(name).exists(),
            "expected source {name} to exist"
        );
    }
    for (name, _) in TEST_CNI_PLUGINS {
        assert!(
            fixture.cni_source_path(name).exists(),
            "expected CNI source {name} to exist"
        );
    }
}

fn assert_all_sources_exist_except(fixture: &Fixture, missing_name: &str) {
    for (name, _) in TEST_BINARIES {
        assert!(
            fixture.source_path(name).exists(),
            "expected source {name} to exist"
        );
    }
    for (name, _) in TEST_CNI_PLUGINS {
        if name == missing_name {
            continue;
        }
        assert!(
            fixture.cni_source_path(name).exists(),
            "expected CNI source {name} to exist"
        );
    }
}

fn assert_all_sources_moved(fixture: &Fixture) {
    for (name, _) in TEST_BINARIES {
        assert!(
            !fixture.source_path(name).exists(),
            "expected source {name} to be moved"
        );
    }
    for (name, _) in TEST_CNI_PLUGINS {
        assert!(
            !fixture.cni_source_path(name).exists(),
            "expected CNI source {name} to be moved"
        );
    }
}

fn assert_no_binaries_installed(fixture: &Fixture) {
    for (name, _) in TEST_BINARIES {
        assert!(
            !fixture.install_path(name).exists(),
            "expected installed {name} to be absent"
        );
    }
    for (name, _) in TEST_CNI_PLUGINS {
        assert!(
            !fixture.cni_install_path(name).exists(),
            "expected installed CNI plugin {name} to be absent"
        );
    }
}

fn assert_all_binaries_installed(fixture: &Fixture) {
    for (name, contents) in TEST_BINARIES {
        assert_eq!(
            fs::read_to_string(fixture.install_path(name))
                .expect("failed to read installed binary"),
            contents
        );
        assert_mode(fixture.install_path(name), 0o755);
    }
    for (name, contents) in TEST_CNI_PLUGINS {
        assert_eq!(
            fs::read_to_string(fixture.cni_install_path(name))
                .expect("failed to read installed CNI plugin"),
            contents
        );
        assert_mode(fixture.cni_install_path(name), 0o755);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}
