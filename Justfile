set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

root := justfile_directory()
containerd_version := `dasel query -i toml -o json 'containerd.version' < vendor.toml | tr -d '"'`
crun_version := `dasel query -i toml -o json 'crun.version' < vendor.toml | tr -d '"'`
cni_plugins_version := `dasel query -i toml -o json 'cni_plugins.version' < vendor.toml | tr -d '"'`
linux_musl_target := `case "$(uname -m)" in arm64|aarch64) echo aarch64-unknown-linux-musl ;; x86_64|amd64) echo x86_64-unknown-linux-musl ;; *) echo "unsupported host architecture: $(uname -m)" >&2; exit 1 ;; esac`
linux_arch := `case "$(uname -m)" in arm64|aarch64) echo arm64 ;; x86_64|amd64) echo amd64 ;; *) echo "unsupported host architecture: $(uname -m)" >&2; exit 1 ;; esac`
containerd_sha256 := `case "$(uname -m)" in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=amd64 ;; *) echo "unsupported host architecture: $(uname -m)" >&2; exit 1 ;; esac; dasel query -i toml -o json "containerd.sha256.${arch}" < vendor.toml | tr -d '"'`
crun_sha256 := `case "$(uname -m)" in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=amd64 ;; *) echo "unsupported host architecture: $(uname -m)" >&2; exit 1 ;; esac; dasel query -i toml -o json "crun.sha256.${arch}" < vendor.toml | tr -d '"'`
cni_plugins_sha256 := `case "$(uname -m)" in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=amd64 ;; *) echo "unsupported host architecture: $(uname -m)" >&2; exit 1 ;; esac; dasel query -i toml -o json "cni_plugins.sha256.${arch}" < vendor.toml | tr -d '"'`
archive := "billow-" + linux_musl_target + ".tar.gz"
stable_archive := "billow.tar.gz"
stage_dir := root + "/target/billow-assemble/" + linux_musl_target
vendor_dir := root + "/.cache/vendor/linux-" + linux_arch + "/containerd-static-" + containerd_version + "-crun-" + crun_version + "-cni-plugins-" + cni_plugins_version
vendor_download_dir := vendor_dir + "/downloads"
vendor_extract_dir := vendor_dir + "/extract"
vendor_bin_dir := vendor_dir + "/bin"
containerd_archive := "containerd-static-" + containerd_version + "-linux-" + linux_arch + ".tar.gz"
containerd_archive_path := vendor_download_dir + "/" + containerd_archive
containerd_url := "https://github.com/containerd/containerd/releases/download/v" + containerd_version + "/" + containerd_archive
crun_download := "crun-" + crun_version + "-linux-" + linux_arch
crun_download_path := vendor_download_dir + "/" + crun_download
crun_url := "https://github.com/containers/crun/releases/download/" + crun_version + "/" + crun_download
cni_plugins_archive := "cni-plugins-linux-" + linux_arch + "-v" + cni_plugins_version + ".tgz"
cni_plugins_archive_path := vendor_download_dir + "/" + cni_plugins_archive
cni_plugins_url := "https://github.com/containernetworking/plugins/releases/download/v" + cni_plugins_version + "/" + cni_plugins_archive
vm_pool_dir := root + "/target/vm-pool"
vm_pool_socket := vm_pool_dir + "/vm-pool.sock"
vm_pool_pid := vm_pool_dir + "/vm-pool.pid"
vm_pool_log := vm_pool_dir + "/vm-pool.log"
vm_pool_bin := root + "/target/debug/vm-pool"

default:
    @just --list

assemble: _build-linux-musl vendor
    mkdir -p "{{stage_dir}}"
    install -m 0755 "{{root}}/target/{{linux_musl_target}}/release/billow-agent" "{{stage_dir}}/billow-agent"
    install -m 0755 "{{root}}/target/{{linux_musl_target}}/release/billow-init" "{{stage_dir}}/billow-init"
    install -m 0755 "{{vendor_bin_dir}}/containerd" "{{stage_dir}}/containerd"
    install -m 0755 "{{vendor_bin_dir}}/containerd-shim-runc-v2" "{{stage_dir}}/containerd-shim-runc-v2"
    install -m 0755 "{{vendor_bin_dir}}/crun" "{{stage_dir}}/crun"
    mkdir -p "{{stage_dir}}/cni"
    install -m 0755 "{{vendor_bin_dir}}/cni/bridge" "{{stage_dir}}/cni/bridge"
    install -m 0755 "{{vendor_bin_dir}}/cni/host-local" "{{stage_dir}}/cni/host-local"
    install -m 0755 "{{vendor_bin_dir}}/cni/loopback" "{{stage_dir}}/cni/loopback"
    COPYFILE_DISABLE=1 tar --no-xattrs -C "{{stage_dir}}" -czf "{{root}}/target/{{archive}}" billow-agent billow-init containerd containerd-shim-runc-v2 crun cni
    cp "{{root}}/target/{{archive}}" "{{root}}/target/{{stable_archive}}"
    install -m 0755 "{{root}}/install.sh" "{{root}}/target/install.sh"
    @printf '%s\n' "WARNING: Billow tarballs are for development and internal testing only. Do not redistribute until third-party license and notice distribution is implemented." >&2
    @echo "{{root}}/target/{{archive}}"
    @echo "{{root}}/target/{{stable_archive}}"

vendor:
    mkdir -p "{{vendor_download_dir}}" "{{vendor_extract_dir}}" "{{vendor_bin_dir}}"
    if [[ ! -f "{{containerd_archive_path}}" ]]; then \
        curl -fL --retry 3 -o "{{containerd_archive_path}}.tmp" "{{containerd_url}}"; \
        printf '%s  %s\n' "{{containerd_sha256}}" "{{containerd_archive_path}}.tmp" | shasum -a 256 -c -; \
        mv "{{containerd_archive_path}}.tmp" "{{containerd_archive_path}}"; \
    fi
    printf '%s  %s\n' "{{containerd_sha256}}" "{{containerd_archive_path}}" | shasum -a 256 -c -
    if [[ ! -x "{{vendor_bin_dir}}/containerd" || ! -x "{{vendor_bin_dir}}/containerd-shim-runc-v2" ]]; then \
        rm -rf "{{vendor_extract_dir}}/containerd"; \
        mkdir -p "{{vendor_extract_dir}}/containerd"; \
        tar -xzf "{{containerd_archive_path}}" -C "{{vendor_extract_dir}}/containerd"; \
        install -m 0755 "{{vendor_extract_dir}}/containerd/bin/containerd" "{{vendor_bin_dir}}/containerd"; \
        install -m 0755 "{{vendor_extract_dir}}/containerd/bin/containerd-shim-runc-v2" "{{vendor_bin_dir}}/containerd-shim-runc-v2"; \
    fi
    if [[ ! -f "{{crun_download_path}}" ]]; then \
        curl -fL --retry 3 -o "{{crun_download_path}}.tmp" "{{crun_url}}"; \
        printf '%s  %s\n' "{{crun_sha256}}" "{{crun_download_path}}.tmp" | shasum -a 256 -c -; \
        mv "{{crun_download_path}}.tmp" "{{crun_download_path}}"; \
    fi
    printf '%s  %s\n' "{{crun_sha256}}" "{{crun_download_path}}" | shasum -a 256 -c -
    if [[ ! -x "{{vendor_bin_dir}}/crun" ]]; then \
        install -m 0755 "{{crun_download_path}}" "{{vendor_bin_dir}}/crun"; \
    fi
    if [[ ! -f "{{cni_plugins_archive_path}}" ]]; then \
        curl -fL --retry 3 -o "{{cni_plugins_archive_path}}.tmp" "{{cni_plugins_url}}"; \
        printf '%s  %s\n' "{{cni_plugins_sha256}}" "{{cni_plugins_archive_path}}.tmp" | shasum -a 256 -c -; \
        mv "{{cni_plugins_archive_path}}.tmp" "{{cni_plugins_archive_path}}"; \
    fi
    printf '%s  %s\n' "{{cni_plugins_sha256}}" "{{cni_plugins_archive_path}}" | shasum -a 256 -c -
    if [[ ! -x "{{vendor_bin_dir}}/cni/bridge" || ! -x "{{vendor_bin_dir}}/cni/host-local" || ! -x "{{vendor_bin_dir}}/cni/loopback" ]]; then \
        rm -rf "{{vendor_extract_dir}}/cni-plugins"; \
        mkdir -p "{{vendor_extract_dir}}/cni-plugins" "{{vendor_bin_dir}}/cni"; \
        tar -xzf "{{cni_plugins_archive_path}}" -C "{{vendor_extract_dir}}/cni-plugins"; \
        install -m 0755 "{{vendor_extract_dir}}/cni-plugins/bridge" "{{vendor_bin_dir}}/cni/bridge"; \
        install -m 0755 "{{vendor_extract_dir}}/cni-plugins/host-local" "{{vendor_bin_dir}}/cni/host-local"; \
        install -m 0755 "{{vendor_extract_dir}}/cni-plugins/loopback" "{{vendor_bin_dir}}/cni/loopback"; \
    fi

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

vm-pool-start:
    mkdir -p "{{vm_pool_dir}}"
    cargo build -p vm-pool
    if BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" "{{vm_pool_bin}}" ping >/dev/null 2>&1; then \
        echo "vm-pool already running (socket: {{vm_pool_socket}})"; \
        exit 0; \
    fi
    rm -f "{{vm_pool_socket}}"
    BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" "{{vm_pool_bin}}" start "{{vm_pool_log}}" "{{vm_pool_pid}}"
    ready=0; \
    for _ in {1..50}; do \
        if BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" "{{vm_pool_bin}}" ping >/dev/null 2>&1; then ready=1; break; fi; \
        sleep 0.1; \
    done; \
    if [[ "$ready" != 1 ]]; then echo "vm-pool did not become ready; see {{vm_pool_log}}" >&2; exit 1; fi; \
    echo "vm-pool started (socket: {{vm_pool_socket}}, log: {{vm_pool_log}})"

vm-pool-wait-ready timeout="600":
    BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" "{{vm_pool_bin}}" wait-ready "{{timeout}}"

vm-pool-status:
    BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" "{{vm_pool_bin}}" status

vm-pool-stop:
    stop_status=0; \
    if [[ -x "{{vm_pool_bin}}" ]] && BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" "{{vm_pool_bin}}" ping >/dev/null 2>&1; then \
        BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" "{{vm_pool_bin}}" stop || stop_status="$?"; \
    else \
        echo "vm-pool is not running"; \
    fi; \
    rm -f "{{vm_pool_socket}}" "{{vm_pool_pid}}"; \
    exit "$stop_status"

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
    BILLOW_VM_POOL_SOCKET="{{vm_pool_socket}}" BILLOW_VM_POOL_BIN="{{vm_pool_bin}}" BILLOW_TEST_PORT="{{port}}" BILLOW_ARCHIVE="{{stable_archive}}" BILLOW_INSTALL_SCRIPT="install.sh" "{{root}}/vm-test.sh"
