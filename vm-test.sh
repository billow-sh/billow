#!/usr/bin/env bash
set -Eeuo pipefail

port="${BILLOW_TEST_PORT:-8000}"
archive="${BILLOW_ARCHIVE:-billow.tar.gz}"
install_script="${BILLOW_INSTALL_SCRIPT:-install.sh}"
vm_name="${BILLOW_VM_NAME:-billow-test-$(date +%s)-$$}"
message="${BILLOW_TEST_MESSAGE:-it works}"
vm_pool_bin="${BILLOW_VM_POOL_BIN:-vm-pool}"
vm_created=0
vm_pool_taken=0

cleanup() {
    status=$?
    set +e

    if [[ "$vm_pool_taken" == 1 ]]; then
        if ! "$vm_pool_bin" drop "$vm_name" >/dev/null 2>&1; then
            multipass stop "$vm_name" >/dev/null 2>&1
            multipass delete --purge "$vm_name" >/dev/null 2>&1
        fi
    elif [[ "$vm_created" == 1 ]]; then
        multipass stop "$vm_name" >/dev/null 2>&1
        multipass delete --purge "$vm_name" >/dev/null 2>&1
    fi

    exit "$status"
}
trap cleanup EXIT

detect_host_ip() {
    if [[ -n "${BILLOW_HOST_IP:-}" ]]; then
        printf '%s\n' "$BILLOW_HOST_IP"
        return
    fi

    if [[ "$vm_created" == 1 ]]; then
        ip="$(multipass exec "$vm_name" -- sh -lc "ip route | awk '/default/ {print \$3; exit}'" 2>/dev/null || true)"
        if [[ -n "$ip" ]]; then
            printf '%s\n' "$ip"
            return
        fi
    fi

    case "$(uname -s)" in
        Darwin)
            for interface in bridge100 bridge101 en0 en1; do
                ip="$(ipconfig getifaddr "$interface" 2>/dev/null || true)"
                if [[ -n "$ip" ]]; then
                    printf '%s\n' "$ip"
                    return
                fi
            done
            ;;
        Linux)
            ip="$(hostname -I 2>/dev/null | awk '{print $1}' || true)"
            if [[ -n "$ip" ]]; then
                printf '%s\n' "$ip"
                return
            fi
            ;;
    esac

    echo "could not determine host IP; set BILLOW_HOST_IP" >&2
    return 1
}

wait_for_vm_exec() {
    for _ in {1..60}; do
        if multipass exec "$vm_name" -- true >/dev/null 2>&1; then
            return
        fi

        sleep 2
    done

    echo "VM did not become ready for multipass exec" >&2
    return 1
}

extract_vm_ip() {
    multipass info "$vm_name" | awk '/IPv4/ {print $2; exit}'
}

vm_pool_available() {
    if [[ "${BILLOW_VM_POOL_DISABLED:-0}" == 1 ]]; then
        return 1
    fi

    if [[ -n "${BILLOW_VM_POOL_BIN:-}" ]]; then
        [[ -x "$vm_pool_bin" ]] || return 1
    else
        command -v "$vm_pool_bin" >/dev/null 2>&1 || return 1
    fi

    "$vm_pool_bin" ping >/dev/null 2>&1
}

if vm_pool_available; then
    echo "Taking Multipass VM from vm-pool" >&2
    if vm_name="$("$vm_pool_bin" take)"; then
        vm_created=1
        vm_pool_taken=1
        echo "Using pooled Multipass VM: $vm_name" >&2
    else
        echo "vm-pool take failed; launching Multipass VM directly" >&2
    fi
fi

if [[ "$vm_created" != 1 ]]; then
    echo "Launching Multipass VM: $vm_name" >&2
    multipass launch --name "$vm_name" --cpus 1 --memory 1G --disk 5G --timeout 600
    vm_created=1
fi

wait_for_vm_exec
host_ip="$(detect_host_ip)"
install_url="http://${host_ip}:${port}/${archive}"
install_script_url="http://${host_ip}:${port}/${install_script}"

echo "Using BILLOW_INSTALL_URL=$install_url" >&2
multipass exec "$vm_name" -- bash -lc "printf '%s\n' 'export BILLOW_INSTALL_URL=$install_url' >> ~/.bashrc"
multipass exec "$vm_name" -- bash -lc "export BILLOW_INSTALL_URL='$install_url'; curl -fsSL '$install_script_url' | bash"

vm_ip="$(extract_vm_ip)"
if [[ -z "$vm_ip" ]]; then
    echo "could not determine VM IP" >&2
    exit 1
fi

run_cli() {
    BILLOW_AGENT_IP="$vm_ip" cargo run --quiet -p billow-cli -- "$@"
}

echo "Testing billow-agent at $vm_ip" >&2
actual=""
for _ in {1..30}; do
    if actual="$(run_cli echo "$message" 2>/dev/null)"; then
        if [[ "$actual" == "$message" ]]; then
            printf '%s\n' "$actual"
            break
        fi
    fi

    sleep 1
done

if [[ "$actual" != "$message" ]]; then
    echo "expected '$message', got '${actual:-<no response>}'" >&2
    exit 1
fi

echo "Testing once workload hello-world" >&2
hello_id="$(run_cli workload submit once hello-world)"
hello_status=""
for _ in {1..120}; do
    if hello_status="$(run_cli workload get "$hello_id" 2>&1)"; then
        if [[ "$hello_status" == *"actual_state=stopped"* || "$hello_status" == *"actual_state=failed"* ]]; then
            break
        fi
    fi

    sleep 2
done

if [[ "$hello_status" != *"actual_state=stopped"* ]]; then
    echo "expected hello-world workload to stop, got '${hello_status:-<no response>}'" >&2
    exit 1
fi

hello_logs=""
if ! hello_logs="$(run_cli workload logs "$hello_id" 2>&1)"; then
    echo "hello-world workload logs failed: ${hello_logs:-<no response>}" >&2
    exit 1
fi
if [[ "$hello_logs" != *"Hello from Docker!"* ]]; then
    echo "expected hello-world output, got '${hello_logs:-<no response>}'" >&2
    exit 1
fi
printf '%s\n' "$hello_logs"

echo "Testing service workload nginx" >&2
nginx_id="$(run_cli workload submit service nginx)"
nginx_status=""
for _ in {1..120}; do
    if nginx_status="$(run_cli workload get "$nginx_id" 2>&1)"; then
        if [[ "$nginx_status" == *"actual_state=running"* ]]; then
            break
        fi
    fi

    sleep 2
done

if [[ "$nginx_status" != *"actual_state=running"* ]]; then
    echo "expected nginx workload to reach running, got '${nginx_status:-<no response>}'" >&2
    exit 1
fi

nginx_logs=""
for _ in {1..60}; do
    if nginx_logs="$(run_cli workload logs "$nginx_id" 2>&1)"; then
        if [[ "$nginx_logs" == *"start worker process"* ]]; then
            break
        fi
    fi

    sleep 2
done

if [[ "$nginx_logs" != *"start worker process"* ]]; then
    echo "expected nginx logs to contain 'start worker process', got '${nginx_logs:-<no response>}'" >&2
    exit 1
fi

if ! nginx_status="$(run_cli workload get "$nginx_id" 2>&1)"; then
    echo "nginx workload status failed: ${nginx_status:-<no response>}" >&2
    exit 1
fi
if [[ "$nginx_status" != *"actual_state=running"* ]]; then
    echo "expected nginx workload to still be running, got '${nginx_status:-<no response>}'" >&2
    exit 1
fi

if ! stop_output="$(run_cli workload stop "$nginx_id" 2>&1)"; then
    echo "nginx workload stop failed: ${stop_output:-<no response>}" >&2
    exit 1
fi

for _ in {1..60}; do
    if nginx_status="$(run_cli workload get "$nginx_id" 2>&1)"; then
        if [[ "$nginx_status" == *"actual_state=stopped"* ]]; then
            break
        fi
    fi

    sleep 2
done

if [[ "$nginx_status" != *"actual_state=stopped"* ]]; then
    echo "expected nginx workload to stop, got '${nginx_status:-<no response>}'" >&2
    exit 1
fi

if ! delete_output="$(run_cli workload delete "$nginx_id" 2>&1)"; then
    echo "nginx workload delete failed: ${delete_output:-<no response>}" >&2
    exit 1
fi

for _ in {1..60}; do
    if nginx_status="$(run_cli workload get "$nginx_id" 2>&1)"; then
        if [[ "$nginx_status" == *"actual_state=deleted"* ]]; then
            break
        fi
    fi

    sleep 2
done

if [[ "$nginx_status" != *"actual_state=deleted"* ]]; then
    echo "expected nginx workload to be deleted, got '${nginx_status:-<no response>}'" >&2
    exit 1
fi

printf '%s\n' "$nginx_status"
