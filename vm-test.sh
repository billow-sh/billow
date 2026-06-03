#!/usr/bin/env bash
set -Eeuo pipefail

port="${BILLOW_TEST_PORT:-8000}"
archive="${BILLOW_ARCHIVE:-billow.tar.gz}"
install_script="${BILLOW_INSTALL_SCRIPT:-install.sh}"
vm_name="${BILLOW_VM_NAME:-billow-test-$(date +%s)-$$}"
message="${BILLOW_TEST_MESSAGE:-it works}"
vm_created=0

cleanup() {
    status=$?
    set +e

    if [[ "$vm_created" == 1 ]]; then
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

echo "Launching Multipass VM: $vm_name" >&2
multipass launch --name "$vm_name" --cpus 1 --memory 1G --disk 5G --timeout 600
vm_created=1

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

echo "Testing billow-agent at $vm_ip" >&2
actual=""
for _ in {1..30}; do
    if actual="$(BILLOW_AGENT_IP="$vm_ip" cargo run --quiet -p billow-cli -- "$message" 2>/dev/null)"; then
        if [[ "$actual" == "$message" ]]; then
            printf '%s\n' "$actual"
            exit 0
        fi
    fi

    sleep 1
done

echo "expected '$message', got '${actual:-<no response>}'" >&2
exit 1
