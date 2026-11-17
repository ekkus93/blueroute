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
# `was-enabled` answers $WAS_ENABLED_EXIT (default: enabled); every other action succeeds.
case "$*" in *was-enabled*) exit "${WAS_ENABLED_EXIT:-0}" ;; esac
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

# What `postinst configure` asks deb-systemd-helper on install and upgrade (QA-A-01).
enable_calls=$'helper unmask blueroute.service\nhelper --quiet was-enabled blueroute.service'
enable_calls+=$'\nhelper enable blueroute.service'
# Fresh install and upgrade: no cleanup, and a stale removal marker is cleared.
fresh_state
printf 'pending\n' > "$state/removal-pending"
check postinst configure "$enable_calls"$'\ninvoke start blueroute.service'
[[ ! -e "$state/removal-pending" ]] || fail 'configure must clear the removal marker'
check prerm upgrade ''
check prerm failed-upgrade ''
check postrm upgrade ''
check postinst configure "$enable_calls"$'\ninvoke restart blueroute.service' '0.1.0'
check postinst triggered ''
# A unit the administrator disabled stays disabled: only its recorded state is updated.
keep_disabled=$'helper unmask blueroute.service\nhelper --quiet was-enabled blueroute.service'
keep_disabled+=$'\nhelper update-state blueroute.service\ninvoke restart blueroute.service'
WAS_ENABLED_EXIT=1 check postinst configure "$keep_disabled" '0.1.0'

# Remove: stop, then clean up, in that order; state is retained.
cleanup=$'invoke stop blueroute.service\ndaemon --remove-owned-networking'
check prerm remove "$cleanup"
check prerm deconfigure "$cleanup"
check postrm remove 'helper mask blueroute.service'
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

# LC-CLEANUP (RFU-02): an unresolved role makes the helper exit 1. Remove, deconfigure and a
# direct purge all stop at prerm, and the membership and client record survive for the
# evidence-based retry. Once the retry's cleanup succeeds, purge deletes them. This checks only
# the maintainer-script contract, because the helper here is a mock. That the real helper exits
# 1 for an unresolved role is asserted by the removal unit tests
# (lc_cleanup_an_unknown_role_is_unresolved_in_every_combination).
fresh_state
printf 'network\tunknown-role\n' > "$state/memberships-v1"
mkdir -p "$state/client-record"
printf 'record\n' > "$state/client-record/record"
for action in remove deconfigure; do
    : > "$BLUEROUTE_MAINTAINER_LOG"
    CLEANUP_EXIT=1 step prerm "$action"
    [[ $status -ne 0 ]] || fail "prerm $action must fail while a role is unresolved"
    check postinst "abort-$action" 'invoke start blueroute.service'
done
CLEANUP_EXIT=1 step prerm remove
[[ $status -ne 0 ]] || fail 'a direct purge must stop at prerm while a role is unresolved'
[[ -s "$state/memberships-v1" && -s "$state/client-record/record" ]] \
    || fail 'an unresolved cleanup must keep the membership and client record'
# QR-RECOVERY (RLC-02): an incomplete manual recovery deletes only the membership. The matching
# record is then an orphan and the real helper still exits 1
# (qr_recovery_deleting_only_the_membership_leaves_an_orphan_record_and_fails), so the retry
# stops at prerm again and the record, the proof for a correct retry, survives.
: > "$state/memberships-v1"
for action in remove deconfigure; do
    : > "$BLUEROUTE_MAINTAINER_LOG"
    CLEANUP_EXIT=1 step prerm "$action"
    [[ $status -ne 0 ]] || fail "prerm $action must fail after an incomplete recovery"
    check postinst "abort-$action" 'invoke start blueroute.service'
done
[[ -s "$state/client-record/record" && -e "$state/node-id" ]] \
    || fail 'an incomplete recovery must keep the client record and identity'
# The documented recovery also deletes the matching record; the real helper then exits 0
# (qr_recovery_the_documented_recovery_of_an_unknown_role_completes), and purge completes.
rm -f "$state/client-record/record"
for script_action in 'prerm remove' 'postrm remove' 'postrm purge'; do
    read -r script action <<< "$script_action"
    step "$script" "$action"
    [[ $status -eq 0 ]] || fail "$script_action failed after recovery"
done
[[ ! -e "$state" ]] || fail 'purge after a successful retry must delete the state directory'

# Remove, then later purge of the removed package: only postrm purge runs then.
fresh_state
step prerm remove
step postrm remove
[[ -e "$state/node-id" ]] || fail 'remove must keep state for a later purge'
check postrm purge $'helper purge blueroute.service\nhelper unmask blueroute.service'
[[ ! -e "$state" ]] || fail 'purge of a removed package must delete the state directory'

printf 'Maintainer lifecycle contract passed (install, upgrade, remove, purge, abort, cleanup failure, unresolved role, manual recovery).\n'
