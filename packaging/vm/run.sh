#!/usr/bin/env bash
# Boots ONE throwaway VM of a distribution from its official cloud image and
# tests TuneMesh packages in it over SSH (see packaging/vm/README.md):
#
#   packaging/vm/run.sh <distro> <package-file>...
#
# Each package is installed, its worker run as a systemd user service and
# self-tested, the app launched in a headless sway session with PipeWire and
# played through its D-Bus actions (playback confirmed by the MPRIS position
# advancing), then uninstalled and checked for leftovers. Packages are tested
# one after another in the same VM.
#
# On a machine with `heavy` (the owner's hosts) start it through heavy:
#
#   setsid --fork --wait heavy --mem 4G --label vm -- packaging/vm/run.sh ...
#
# Only one VM runs at a time: a second run.sh refuses to start while the lock
# is held. No part of this script starts a second VM.
#
# Environment:
#   TM_VM_CACHE  downloaded images (default ${XDG_CACHE_HOME:-~/.cache}/tunemesh-vm)
#   TM_VM_LOGS   where logs are copied (default ./vm-logs-<distro>)
#   TM_VM_LOCK   lock file (default /tmp/tunemesh-vm.lock)
#   TM_VM_ACCEL  kvm or tcg (default kvm when /dev/kvm is usable, else tcg)
#   TM_VM_KEEP=1 keep the VM's work directory (disk overlay, SSH key)
#   TM_VM_HOLD=1 after a failure, keep the VM running for inspection over SSH
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
distros="ubuntu-24.04 ubuntu-26.04 debian-13 fedora arch"

usage() {
    echo "usage: $0 <distro> <package-file>..." >&2
    echo "  distro: ${distros// /|}" >&2
    echo "  package: .deb .rpm .pkg.tar.zst .AppImage .snap .flatpak" >&2
    exit 2
}
log() { printf '[vm %(%H:%M:%S)T] %s\n' -1 "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

[ $# -ge 2 ] || usage
distro=$1
shift
case " $distros " in *" $distro "*) ;; *) usage ;; esac
packages=()
for p in "$@"; do
    [ -f "$p" ] || die "no such package file: $p"
    case $p in
    *.deb | *.rpm | *.pkg.tar.zst | *.AppImage | *.snap | *.flatpak) ;;
    *) die "unknown package type: $p" ;;
    esac
    packages+=("$(cd "$(dirname "$p")" && pwd)/$(basename "$p")")
done

for tool in qemu-system-x86_64 qemu-img ssh scp ssh-keygen curl ffmpeg python3 flock tar gpg gpgv; do
    command -v "$tool" >/dev/null || die "$tool is missing"
done
iso_tool=
for t in cloud-localds xorriso genisoimage mkisofs; do
    if command -v "$t" >/dev/null; then iso_tool=$t; break; fi
done
[ -n "$iso_tool" ] || die "need one of cloud-localds, xorriso, genisoimage or mkisofs for the cloud-init seed"

# Through heavy on hosts that have it, so a VM never overloads the machine.
if [ -z "${GITHUB_ACTIONS:-}" ] && command -v heavy >/dev/null && ! grep -q 'heavy' /proc/self/cgroup; then
    die "start this through heavy: setsid --fork --wait heavy --mem 4G --label vm -- $0 $distro $*"
fi

# ---------------------------------------------------------------- one VM only
lock=${TM_VM_LOCK:-/tmp/tunemesh-vm.lock}
exec 9>>"$lock"
if ! flock -n 9; then
    die "another packaging/vm/run.sh VM is running (lock $lock held); run one VM at a time"
fi

cache=${TM_VM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tunemesh-vm}
logs=${TM_VM_LOGS:-$PWD/vm-logs-$distro}
mkdir -p "$cache" "$logs"
logs=$(cd "$logs" && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/tunemesh-vm.XXXXXX")
qemu_pid=

# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    status=$?
    if [ -n "$qemu_pid" ] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill "$qemu_pid" 2>/dev/null || true
        for _ in $(seq 20); do kill -0 "$qemu_pid" 2>/dev/null || break; sleep 0.5; done
        kill -9 "$qemu_pid" 2>/dev/null || true
    fi
    [ -f "$work/console.log" ] && cp "$work/console.log" "$logs/console.log"
    if [ "${TM_VM_KEEP:-}" = 1 ]; then log "kept $work"; else rm -rf "$work"; fi
    if [ "$status" -eq 0 ]; then log "PASS ($distro); logs in $logs"; else log "FAIL ($distro); logs in $logs"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

# ---------------------------------------------------------------- the image
read -r url algo sum < <("$here/images.sh" "$distro")
[ -n "${sum:-}" ] || die "could not resolve the $distro image"
image=$cache/$distro-${sum:0:16}.qcow2
verify() { echo "$sum  $1" | "${algo}sum" --quiet -c -; }
if [ -f "$image" ] && verify "$image"; then
    log "image $image (cached, $algo verified)"
else
    rm -f "$image"
    # Bounded: two attempts, each with curl's own three retries.
    for attempt in 1 2; do
        log "downloading $url (attempt $attempt)"
        if curl -fL --retry 3 --retry-delay 10 --connect-timeout 30 -o "$image.part" "$url" &&
            verify "$image.part"; then
            mv "$image.part" "$image"
            break
        fi
        rm -f "$image.part"
        [ $attempt -eq 2 ] && die "download or $algo check of $url failed twice"
        sleep 20
    done
    # Older images of the same distro are of no further use.
    find "$cache" -maxdepth 1 -name "$distro-*.qcow2" ! -path "$image" -delete
    log "image $image ($algo verified)"
fi

# ---------------------------------------------------------------- seed + disk
ssh-keygen -q -t ed25519 -N '' -C tunemesh-vm -f "$work/id"
# The guest's SSH host key is made here and handed over in the seed, so the
# host can pin it (known_hosts) instead of trusting whatever answers on the port.
ssh-keygen -q -t ed25519 -N '' -C tunemesh-vm-host -f "$work/host"
printf 'tunemesh-vm %s\n' "$(cut -d' ' -f1-2 "$work/host.pub")" >"$work/known_hosts"
cat >"$work/meta-data" <<EOF
instance-id: tunemesh-vm-$$
local-hostname: tunemesh-vm
EOF
cat >"$work/user-data" <<EOF
#cloud-config
users:
  - name: tm
    gecos: TuneMesh test user
    shell: /bin/bash
    lock_passwd: true
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - $(cat "$work/id.pub")
ssh_pwauth: false
# Only the host key made by run.sh.
ssh_deletekeys: true
ssh_genkeytypes: []
ssh_keys:
  ed25519_private: |
$(sed 's/^/    /' "$work/host")
  ed25519_public: $(cat "$work/host.pub")
runcmd:
  # The user's systemd starts at boot and outlives SSH sessions.
  - [loginctl, enable-linger, tm]
EOF
case $iso_tool in
cloud-localds) cloud-localds "$work/seed.iso" "$work/user-data" "$work/meta-data" ;;
xorriso) xorriso -as mkisofs -quiet -o "$work/seed.iso" -V cidata -J -r "$work/user-data" "$work/meta-data" ;;
*) "$iso_tool" -quiet -o "$work/seed.iso" -V cidata -J -r "$work/user-data" "$work/meta-data" ;;
esac
qemu-img create -q -f qcow2 -F qcow2 -b "$image" "$work/disk.qcow2" 24G

# A generated library: tagged tones, never real music (AGENTS.md).
lib=$work/payload/library
mkdir -p "$lib/TuneMesh Test Artist/Tone Album" "$lib/Playlists" "$lib/.tunemesh/requests"
for n in 1 2; do
    ffmpeg -nostdin -loglevel error -f lavfi -i "sine=frequency=$((330 * n)):duration=60" \
        -c:a libopus -b:a 48k -metadata title="Tone $n" -metadata artist="TuneMesh Test Artist" \
        -metadata album="Tone Album" -metadata track="$n" \
        "$lib/TuneMesh Test Artist/Tone Album/Tone $n.opus"
done
printf '#EXTM3U\n../TuneMesh Test Artist/Tone Album/Tone 1.opus\n' >"$lib/Playlists/Test.m3u8"
cp "$here/guest.sh" "$repo/packaging/ci/check-removed.sh" "$work/payload/"

# ---------------------------------------------------------------- boot
accel=${TM_VM_ACCEL:-}
if [ -z "$accel" ]; then
    if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then accel=kvm; else accel=tcg; fi
fi
if [ "$accel" = kvm ]; then
    cpu=(-accel kvm -cpu host)
else
    log "WARNING: /dev/kvm is not usable; falling back to TCG emulation (much slower)"
    cpu=(-accel "tcg,thread=multi" -cpu max)
fi
# QEMU binds the SSH forward itself; a free port picked beforehand can be
# taken by someone else before QEMU binds it, so a failed bind is retried
# with a new port (bounded). Whoever answers on the port must still present
# the pinned host key.
port_free() {
    python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
port_open() {
    python3 -c 'import socket, sys; s = socket.socket(); s.settimeout(1); sys.exit(s.connect_ex(("127.0.0.1", int(sys.argv[1]))))' "$1"
}
for attempt in 1 2 3 4 5; do
    port=$(port_free)
    log "booting $distro ($accel, 2 vCPU, 3 GiB), SSH on 127.0.0.1:$port"
    qemu-system-x86_64 -machine q35 "${cpu[@]}" -smp 2 -m 3072 \
        -drive "file=$work/disk.qcow2,if=virtio,format=qcow2,discard=unmap" \
        -drive "file=$work/seed.iso,media=cdrom,readonly=on" \
        -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:$port-:22" -device virtio-net-pci,netdev=net0 \
        -device virtio-rng-pci -display none -monitor none -serial "file:$work/console.log" \
        2>"$work/qemu.log" &
    qemu_pid=$!
    # QEMU listens on the forward before the guest runs: wait for that.
    for _ in $(seq 40); do
        kill -0 "$qemu_pid" 2>/dev/null || break
        port_open "$port" && break
        sleep 0.25
    done
    sleep 0.5
    kill -0 "$qemu_pid" 2>/dev/null && break
    wait "$qemu_pid" 2>/dev/null || true
    qemu_pid=
    cat "$work/qemu.log" >&2
    grep -qi 'hostfwd\|host forwarding' "$work/qemu.log" || die "QEMU did not start (see above)"
    [ $attempt -lt 5 ] || die "QEMU could not bind an SSH port in 5 attempts"
    log "port $port was taken; retrying with another"
done

ssh_opts=(-i "$work/id" -o HostKeyAlias=tunemesh-vm -o UserKnownHostsFile="$work/known_hosts"
    -o StrictHostKeyChecking=yes -o HostKeyAlgorithms=ssh-ed25519 -o LogLevel=ERROR
    -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=8)
vm() { ssh "${ssh_opts[@]}" -p "$port" tm@127.0.0.1 "$@"; }

boot_limit=300
[ "$accel" = tcg ] && boot_limit=1200
start=$SECONDS
until vm true 2>/dev/null; do
    kill -0 "$qemu_pid" 2>/dev/null || die "QEMU exited during boot (see console.log)"
    [ $((SECONDS - start)) -lt $boot_limit ] || die "no SSH after ${boot_limit}s (see console.log)"
    sleep 5
done
# cloud-init status exits 2 for recoverable errors (deprecated keys and such).
# shellcheck disable=SC2016 # expanded in the VM
vm 'sudo cloud-init status --wait >/dev/null; s=$?; [ $s -eq 0 ] || [ $s -eq 2 ]' ||
    die "cloud-init failed"
# shellcheck disable=SC2016 # expanded in the VM
log "booted in $((SECONDS - start))s: $(vm '. /etc/os-release; echo "$PRETTY_NAME, kernel $(uname -r)"')"

# ---------------------------------------------------------------- test
vm 'mkdir -p ~/tm/pkg ~/tm/logs'
tar -C "$work/payload" -cf - . | vm 'tar -C ~/tm -xf -'
scp "${ssh_opts[@]}" -P "$port" -q "${packages[@]}" tm@127.0.0.1:tm/pkg/

result=0
collect() {
    vm 'sudo journalctl -b --no-pager >~/tm/logs/journal-system.log 2>&1;
        journalctl --user -b --no-pager >~/tm/logs/journal-user.log 2>&1; true' || true
    vm 'tar -C ~/tm/logs -cf - .' | tar -C "$logs" -xf - || true
}
if ! vm "bash ~/tm/guest.sh deps $distro" 2>&1 | tee "$logs/deps.log"; then
    collect
    die "installing the test dependencies failed"
fi
for p in "${packages[@]}"; do
    name=$(basename "$p")
    log "testing $name"
    if vm "bash ~/tm/guest.sh test $distro ~/tm/pkg/$(printf %q "$name")" 2>&1 | tee "$logs/test-$name.log"; then
        log "$name: PASS"
    else
        log "$name: FAIL"
        result=1
    fi
done
collect
if [ "${TM_VM_HOLD:-}" = 1 ] && [ $result -ne 0 ]; then
    touch "$work/hold"
    log "TM_VM_HOLD: VM kept running; log in with"
    log "  ssh -i $work/id -p $port -o HostKeyAlias=tunemesh-vm -o UserKnownHostsFile=$work/known_hosts tm@127.0.0.1"
    log "  and rm $work/hold to shut it down"
    while [ -e "$work/hold" ] && kill -0 "$qemu_pid" 2>/dev/null; do sleep 5; done
fi
vm 'sudo systemctl poweroff' 2>/dev/null || true
for _ in $(seq 60); do kill -0 "$qemu_pid" 2>/dev/null || break; sleep 1; done
exit $result
