#!/bin/bash
# SCAFFOLDING - branch fix/linux-seek-audio only.
#
# Brings up what the harness needs in a session that has none of it: a running
# PipeWire, two null sinks to record, and a compositor for the GTK window.
# Run once per boot; re-running is harmless.
#
# Prints the environment to export, so a caller can do:
#   eval "$(./startenv.sh)"
set -eu

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

log() { echo "$@" >&2; }

# PipeWire, started by hand rather than through systemd: a VM reached over SSH
# often has no user session bus, and the harness only needs the daemons, not a
# desktop.
if ! pactl info >/dev/null 2>&1; then
    log "starting pipewire"
    pipewire >/tmp/pipewire.log 2>&1 &
    sleep 1
    wireplumber >/tmp/wireplumber.log 2>&1 &
    pipewire-pulse >/tmp/pipewire-pulse.log 2>&1 &
    for _ in $(seq 20); do
        pactl info >/dev/null 2>&1 && break
        sleep 0.5
    done
fi
pactl info >/dev/null 2>&1 || {
    log "pipewire did not come up - see /tmp/pipewire*.log"
    exit 1
}

# The two sinks the harness records. Matched by description, not node name.
for s in a b; do
    name="tp_$s"
    desc="TP_$(echo "$s" | tr '[:lower:]' '[:upper:]')"
    if ! pactl list short sinks | grep -q "	$name	"; then
        log "creating sink $name"
        pactl load-module module-null-sink \
            sink_name="$name" sink_properties=device.description="$desc" >/dev/null
    fi
done

# A compositor, if the session has no Wayland display already. WSL supplies one
# through WSLg; a Lima VM supplies nothing, so weston runs headless there.
if [ ! -S "$XDG_RUNTIME_DIR/${WAYLAND_DISPLAY:-wayland-0}" ]; then
    if [ -S /mnt/wslg/runtime-dir/wayland-0 ]; then
        log "linking WSLg's wayland socket"
        ln -sf /mnt/wslg/runtime-dir/wayland-0 "$XDG_RUNTIME_DIR/wayland-0"
    else
        log "starting headless weston"
        weston --backend=headless --width=1920 --height=1080 \
            >/tmp/weston.log 2>&1 &
        for _ in $(seq 20); do
            [ -S "$XDG_RUNTIME_DIR/wayland-1" ] && break
            [ -S "$XDG_RUNTIME_DIR/wayland-0" ] && break
            sleep 0.5
        done
    fi
fi

display=wayland-0
[ -S "$XDG_RUNTIME_DIR/wayland-1" ] && display=wayland-1
[ -S "$XDG_RUNTIME_DIR/$display" ] || {
    log "no wayland display - see /tmp/weston.log"
    exit 1
}

log "ready: $display, sinks $(pactl list short sinks | grep -c '	tp_')"
echo "export XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR WAYLAND_DISPLAY=$display"
