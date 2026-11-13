#!/usr/bin/env bash
# Unprivileged Debian maintainer-script lifecycle contract checks (including CR-F5 removal
# cleanup). The real scripts run unchanged with DPKG_ROOT pointing at a temporary root, exactly
# as dpkg runs them for a chrootless install; the daemon executable there is a mock.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
script_dir="${BLUEROUTE_MAINTAINER_SCRIPT_DIR:-$root/packaging/debian}"
for script in postinst prerm postrm; do
    test -f "$script_dir/$script" || { printf "Missing maintainer script: %s\\n" "$script_dir/$script" >&2; exit 1; }
done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/root/usr/libexec/blueroute"
cat > "$tmp/bin/deb-systemd-helper" <<'MOCK'
#!/bin/sh
printf 'helper %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
MOCK
cat > "$tmp/bin/deb-systemd-invoke" <<'MOCK'
#!/bin/sh
printf 'invoke %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
MOCK
# Stands in for the packaged daemon; exits with $CLEANUP_EXIT.
cat > "$tmp/root/usr/libexec/blueroute/blueroute-daemon" <<'MOCK'
#!/bin/sh
printf 'daemon %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
[ "$BLUEROUTE_STATE_DIR" = "$DPKG_ROOT/var/lib/blueroute" ] || {
    echo "cleanup got the wrong state directory: $BLUEROUTE_STATE_DIR" >&2
    exit 3
}
exit "${CLEANUP_EXIT:-0}"
MOCK
chmod +x "$tmp/bin/"* "$tmp/root/usr/libexec/blueroute/blueroute-daemon"
export PATH="$tmp/bin:$PATH"
export BLUEROUTE_MAINTAINER_LOG="$tmp/commands"
export DPKG_ROOT="$tmp/root"
state="$DPKG_ROOT/var/lib/blueroute"
for script in postinst prerm postrm; do
    # Every fixed path must be under DPKG_ROOT, or a chrootless run would touch the host.
    if grep -v '^\s*#' "$script_dir/$script" | grep -E '(^|[^}])/(usr|var|etc|run)/' >/dev/null; then
        printf '%s uses a fixed path outside ${DPKG_ROOT:-}\n' "$script" >&2
        exit 1
    fi
done

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

# Run one script; record its exit status in $status.
step() {
    local script="$1"
    shift
    set +e
    sh "$script_dir/$script" "$@"
    status=$?
    set -e
}

check() {
    local script="$1" action="$2" expected="$3"
    shift 3
    : > "$BLUEROUTE_MAINTAINER_LOG"
    step "$script" "$action" "$@"
    [[ $status -eq 0 ]] || fail "$script $action exited $status"
    local actual
    actual="$(cat "$BLUEROUTE_MAINTAINER_LOG")"
    [[ "$actual" == "$expected" ]] \
        || fail "Unexpected $script $action calls:"$'\n'"Expected: $expected"$'\n'"Actual: $actual"
}

fresh_state() {
    rm -rf "$state"
    mkdir -p "$state"
    printf 'identity\n' > "$state/node-id"
}

# Fresh install and upgrade: no cleanup, and a stale removal marker is cleared.
fresh_state
printf 'pending\n' > "$state/removal-pending"
check postinst configure $'helper enable blueroute.service\ninvoke start blueroute.service'
[[ ! -e "$state/removal-pending" ]] || fail 'configure must clear the removal marker'
check prerm upgrade ''
check prerm failed-upgrade ''
check postrm upgrade ''
check postinst configure $'helper enable blueroute.service\ninvoke restart blueroute.service' '0.1.0'
check postinst triggered ''

# Remove: stop, then clean up, in that order; state is retained.
cleanup=$'invoke stop blueroute.service\ndaemon --remove-owned-networking'
check prerm remove "$cleanup"
check prerm deconfigure "$cleanup"
check postrm remove 'helper disable blueroute.service'
[[ -e "$state/node-id" ]] || fail 'remove must retain durable state'
check postrm disappear ''

# A failed cleanup fails prerm, so dpkg aborts the removal; abort-remove restores the service.
fresh_state
printf 'pending\n' > "$state/removal-pending"
: > "$BLUEROUTE_MAINTAINER_LOG"
CLEANUP_EXIT=1 step prerm remove
[[ $status -ne 0 ]] || fail 'prerm remove must fail when cleanup fails'
check postinst abort-remove 'invoke start blueroute.service'
[[ ! -e "$state/removal-pending" ]] || fail 'abort-remove must clear the removal marker'
[[ -e "$state/node-id" ]] || fail 'an aborted removal must keep durable state'
printf 'pending\n' > "$state/removal-pending"
check postinst abort-deconfigure 'invoke start blueroute.service'
[[ ! -e "$state/removal-pending" ]] || fail 'abort-deconfigure must clear the removal marker'

# Direct purge of an installed package, as dpkg sequences it: cleanup precedes deletion.
fresh_state
: > "$BLUEROUTE_MAINTAINER_LOG"
for script_action in 'prerm remove' 'postrm remove' 'postrm purge'; do
    read -r script action <<< "$script_action"
    step "$script" "$action"
    [[ $status -eq 0 ]] || fail "$script_action failed during purge"
done
[[ ! -e "$state" ]] || fail 'purge must delete the state directory'
grep -n . "$BLUEROUTE_MAINTAINER_LOG" | grep -q '^2:daemon --remove-owned-networking$' \
    || fail 'purge must run cleanup right after stopping the service'

# Direct purge whose cleanup fails: dpkg stops at prerm, so the state is never deleted.
fresh_state
CLEANUP_EXIT=1 step prerm remove
[[ $status -ne 0 ]] || fail 'prerm must fail'
[[ -e "$state/node-id" ]] || fail 'a failed cleanup must not lose the state needed to retry'

# Remove, then later purge of the removed package: only postrm purge runs then.
fresh_state
step prerm remove
step postrm remove
[[ -e "$state/node-id" ]] || fail 'remove must keep state for a later purge'
check postrm purge ''
[[ ! -e "$state" ]] || fail 'purge of a removed package must delete the state directory'

printf 'Maintainer lifecycle contract passed (install, upgrade, remove, purge, abort, cleanup failure).\n'
