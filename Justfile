set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

root := justfile_directory()
linux_musl_target := `case "$(uname -m)" in arm64|aarch64) echo aarch64-unknown-linux-musl ;; x86_64|amd64) echo x86_64-unknown-linux-musl ;; *) echo "unsupported host architecture: $(uname -m)" >&2; exit 1 ;; esac`
archive := "billow-" + linux_musl_target + ".tar.gz"
stable_archive := "billow.tar.gz"
stage_dir := root + "/target/billow-assemble/" + linux_musl_target

default:
    @just --list

assemble: _build-linux-musl
    mkdir -p "{{stage_dir}}"
    install -m 0755 "{{root}}/target/{{linux_musl_target}}/release/billow-agent" "{{stage_dir}}/billow-agent"
    install -m 0755 "{{root}}/target/{{linux_musl_target}}/release/billow-init" "{{stage_dir}}/billow-init"
    COPYFILE_DISABLE=1 tar --no-xattrs -C "{{stage_dir}}" -czf "{{root}}/target/{{archive}}" billow-agent billow-init
    cp "{{root}}/target/{{archive}}" "{{root}}/target/{{stable_archive}}"
    install -m 0755 "{{root}}/install.sh" "{{root}}/target/install.sh"
    @echo "{{root}}/target/{{archive}}"
    @echo "{{root}}/target/{{stable_archive}}"

_build-linux-musl:
    case "$(uname -s)" in \
        Darwin) cargo zigbuild --workspace --release --target {{linux_musl_target}} ;; \
        Linux) cargo build --workspace --release --target {{linux_musl_target}} ;; \
        *) cargo build --workspace --release --target {{linux_musl_target}} ;; \
    esac

serve port="8000" bind="0.0.0.0": assemble
    @echo "Serving http://{{bind}}:{{port}}/{{stable_archive}}"
    @echo "Installer: http://{{bind}}:{{port}}/install.sh"
    python3 -m http.server {{port}} --bind {{bind}} --directory "{{root}}/target"

vm-test port="8000": assemble
    server_log="{{root}}/target/vm-test-http.log"; \
    python3 -m http.server "{{port}}" --bind 0.0.0.0 --directory "{{root}}/target" > "$server_log" 2>&1 & \
    server_pid="$!"; \
    cleanup() { kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; }; \
    trap cleanup EXIT; \
    ready=0; \
    for _ in {1..30}; do \
        if curl -fsS -o /dev/null "http://127.0.0.1:{{port}}/install.sh"; then ready=1; break; fi; \
        sleep 0.25; \
    done; \
    if [[ "$ready" != 1 ]]; then echo "HTTP server did not become ready; see $server_log" >&2; exit 1; fi; \
    BILLOW_TEST_PORT="{{port}}" BILLOW_ARCHIVE="{{stable_archive}}" BILLOW_INSTALL_SCRIPT="install.sh" "{{root}}/vm-test.sh"
