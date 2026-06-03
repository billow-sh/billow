use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const INIT_BIN: &str = env!("CARGO_BIN_EXE_billow-init");

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    download_dir: PathBuf,
    install_bin_dir: PathBuf,
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
        let systemd_unit_dir = root.join("etc-systemd-system");
        let systemd_runtime_dir = root.join("run-systemd-system");
        let fake_bin_dir = root.join("fake-bin");
        let empty_path_dir = root.join("empty-path");

        for dir in [
            &download_dir,
            &install_bin_dir,
            &systemd_unit_dir,
            &systemd_runtime_dir,
            &fake_bin_dir,
            &empty_path_dir,
        ] {
            fs::create_dir_all(dir).expect("failed to create fixture directory");
        }

        fs::write(download_dir.join("billow-init"), "fake init\n")
            .expect("failed to create fake init");
        fs::write(download_dir.join("billow-agent"), "fake agent\n")
            .expect("failed to create fake agent");
        write_fake_systemctl(&fake_bin_dir).expect("failed to create fake systemctl");

        Self {
            root,
            download_dir,
            install_bin_dir,
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
        self.download_dir.join("billow-agent")
    }

    fn agent_install_path(&self) -> PathBuf {
        self.install_bin_dir.join("billow-agent")
    }

    fn service_path(&self) -> PathBuf {
        self.systemd_unit_dir.join("billow-agent.service")
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
    assert!(!fixture.agent_source_path().exists());
    assert_eq!(
        fs::read_to_string(fixture.agent_install_path()).expect("failed to read installed agent"),
        "fake agent\n"
    );
    assert_mode(fixture.agent_install_path(), 0o755);

    let unit = fs::read_to_string(fixture.service_path()).expect("failed to read service unit");
    assert!(unit.contains("Description=Billow Agent"));
    assert!(unit.contains(&format!(
        "ExecStart={}",
        fixture.agent_install_path().display()
    )));
    assert!(unit.contains("Restart=on-failure"));
    assert_mode(fixture.service_path(), 0o644);

    assert_eq!(
        fixture.systemctl_log(),
        "--version\ndaemon-reload\nenable --now billow-agent.service\n"
    );
    assert!(stdout(&output).contains("billow-agent installed and started as billow-agent.service"));
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
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.agent_install_path().exists());
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
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.agent_install_path().exists());
    assert_eq!(fixture.systemctl_log(), "");
}

#[test]
fn fails_when_systemd_unit_dir_is_missing() {
    let fixture = Fixture::new();
    fs::remove_dir_all(&fixture.systemd_unit_dir).expect("failed to remove systemd unit dir");

    let output = fixture.run();

    assert_failure_contains(&output, "systemd unit directory");
    assert_failure_contains(&output, "does not exist");
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.agent_install_path().exists());
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
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.agent_install_path().exists());
    assert_eq!(fixture.systemctl_log(), "");
}

#[test]
fn fails_when_systemctl_version_fails() {
    let fixture = Fixture::new();
    fixture.fail_systemctl_command("--version");

    let output = fixture.run();

    assert_failure_contains(&output, "systemctl is not available");
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.agent_install_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_systemd_runtime_dir_is_missing() {
    let fixture = Fixture::new();
    fs::remove_dir_all(&fixture.systemd_runtime_dir).expect("failed to remove runtime dir");

    let output = fixture.run();

    assert_failure_contains(&output, "systemd runtime directory");
    assert_failure_contains(&output, "does not exist");
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.agent_install_path().exists());
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
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_service_is_already_installed() {
    let fixture = Fixture::new();
    fs::write(fixture.service_path(), "installed already\n")
        .expect("failed to create installed unit");

    let output = fixture.run();

    assert_failure_contains(&output, "billow-agent.service already exists");
    assert!(fixture.agent_source_path().exists());
    assert!(!fixture.agent_install_path().exists());
    assert_eq!(
        fs::read_to_string(fixture.service_path()).expect("failed to read service unit"),
        "installed already\n"
    );
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_agent_source_is_missing() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.agent_source_path()).expect("failed to remove source agent");

    let output = fixture.run();

    assert_failure_contains(&output, "billow-agent must be present");
    assert!(!fixture.agent_install_path().exists());
    assert!(!fixture.service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\n");
}

#[test]
fn fails_when_daemon_reload_fails() {
    let fixture = Fixture::new();
    fixture.fail_systemctl_command("daemon-reload");

    let output = fixture.run();

    assert_failure_contains(&output, "systemctl daemon-reload failed");
    assert!(!fixture.agent_source_path().exists());
    assert!(fixture.agent_install_path().exists());
    assert!(fixture.service_path().exists());
    assert_eq!(fixture.systemctl_log(), "--version\ndaemon-reload\n");
}

#[test]
fn fails_when_service_start_fails() {
    let fixture = Fixture::new();
    fixture.fail_systemctl_command("enable");

    let output = fixture.run();

    assert_failure_contains(
        &output,
        "systemctl enable --now billow-agent.service failed",
    );
    assert!(!fixture.agent_source_path().exists());
    assert!(fixture.agent_install_path().exists());
    assert!(fixture.service_path().exists());
    assert_eq!(
        fixture.systemctl_log(),
        "--version\ndaemon-reload\nenable --now billow-agent.service\n"
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}
