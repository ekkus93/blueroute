#!/usr/bin/env bash
# Service enablement across the package lifecycle (QA-A-01), with the REAL deb-systemd-helper and
# `systemctl --root` working on a temporary root (DPKG_ROOT), as dpkg runs a chrootless install.
# Only deb-systemd-invoke (it would talk to the running systemd) and the cleanup helper are mocked.
# dpkg's file handling is imitated: the unit file is unpacked before `postinst configure` and
# deleted between `prerm remove` and `postrm remove`.
#
# CR-QUALIFY Part A found that remove-then-reinstall left the service disabled and stopped;
# the maintainer-script tests could not see it because they mock deb-systemd-helper.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
script_dir="${BLUEROUTE_MAINTAINER_SCRIPT_DIR:-$root/packaging/debian}"
unit_source="$root/packaging/systemd/blueroute.service"
for tool in deb-systemd-helper systemctl; do
    command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
cat > "$tmp/bin/deb-systemd-invoke" <<'MOCK'
#!/bin/sh
printf 'invoke %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
MOCK
chmod +x "$tmp/bin/deb-systemd-invoke"
export PATH="$tmp/bin:$PATH"
export BLUEROUTE_MAINTAINER_LOG="$tmp/commands"
export DPKG_ROOT="$tmp/root"
unit="$DPKG_ROOT/usr/lib/systemd/system/blueroute.service"
wants="$DPKG_ROOT/etc/systemd/system/multi-user.target.wants/blueroute.service"
mask="$DPKG_ROOT/etc/systemd/system/blueroute.service"
state="$DPKG_ROOT/var/lib/systemd/deb-systemd-helper-enabled"
daemon="$DPKG_ROOT/usr/libexec/blueroute/blueroute-daemon"
# deb-systemd-helper takes its systemd code path (`systemctl preset` on a first install, links
# only for units that already have some) only when the target root has systemctl. Without it, it
# always creates the links, which would hide QA-A-01.
mkdir -p "$DPKG_ROOT/usr/bin"
ln -s "$(command -v systemctl)" "$DPKG_ROOT/usr/bin/systemctl"

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

export DPKG_MAINTSCRIPT_PACKAGE=blueroute
run() {
    : > "$BLUEROUTE_MAINTAINER_LOG"
    # dpkg's environment; deb-systemd-helper refuses to run without it.
    DPKG_MAINTSCRIPT_NAME="$1" sh "$script_dir/$1" "${@:2}" 2> "$tmp/stderr" \
        || fail "$1 ${*:2} exited $?: $(cat "$tmp/stderr")"
}

unpack() {
    install -D -m 0644 "$unit_source" "$unit"
    mkdir -p "$(dirname "$daemon")"
    printf '#!/bin/sh\nexit 0\n' > "$daemon"
    chmod +x "$daemon"
}

delete_files() {
    rm -f "$unit" "$daemon"
}

enabled() {
    [[ -L $wants && $(readlink "$wants") == */blueroute.service ]]
}

masked() {
    [[ -L $mask && $(readlink "$mask") == /dev/null ]]
}

expect_started() {
    local verb="$1" context="$2"
    enabled || fail "$context: the unit must be enabled"
    ! masked || fail "$context: the unit must not be masked"
    grep -Fxq "invoke $verb blueroute.service" "$BLUEROUTE_MAINTAINER_LOG" \
        || fail "$context: postinst must $verb the service"
}

# First install: enabled and started.
unpack
run postinst configure
expect_started start 'first install'

# Remove: the unit file goes, the unit is masked and keeps its enablement for a reinstall.
run prerm remove
delete_files
run postrm remove
masked || fail 'remove: the unit must be masked so nothing starts it without its files'

# Reinstall after remove (QA-A-01): enabled again, unmasked and started.
unpack
run postinst configure
expect_started start 'reinstall after remove'

# Upgrade: still enabled; restarted on the new binary.
run prerm upgrade 0.1.1
run postinst configure 0.1.0
expect_started restart 'upgrade'

# An administrator's disable survives an upgrade and a remove-then-reinstall.
systemctl --root="$DPKG_ROOT" disable blueroute.service >/dev/null 2>&1
! enabled || fail 'precondition: systemctl disable must remove the link'
run postinst configure 0.1.0
! enabled || fail 'upgrade must keep an administrator-disabled unit disabled'
run prerm remove
delete_files
run postrm remove
unpack
run postinst configure
! enabled || fail 'reinstall must keep an administrator-disabled unit disabled'
! masked || fail 'reinstall must unmask the unit'

# Purge forgets the unit: no link, no mask, no deb-systemd-helper record.
systemctl --root="$DPKG_ROOT" enable blueroute.service >/dev/null 2>&1
run prerm remove
delete_files
run postrm remove
run postrm purge
! enabled || fail 'purge must remove the enablement link'
! masked || fail 'purge must unmask the unit'
if find "$state" -name '*blueroute*' 2>/dev/null | grep -q .; then
    fail "purge must remove deb-systemd-helper's state: $(find "$state" -name '*blueroute*')"
fi

# After a purge, an install is a first install again.
unpack
run postinst configure
expect_started start 'install after purge'

printf '%s\n' 'Service enablement lifecycle passed with the real deb-systemd-helper' \
    '(install, remove, reinstall, upgrade, administrator disable, purge).'
