#!/usr/bin/env bash
# Exercise an actual dpkg upgrade in an isolated package database and filesystem.
# No host package installation, systemd operation, or Bluetooth device is required.
set -euo pipefail
if [[ $# -ne 1 || ! -f $1 ]]; then
    echo "Usage: $0 path/to/blueroute_VERSION_ARCH.deb" >&2
    exit 2
fi
package="$(realpath -- "$1")"
tmp="$(mktemp -d)"
trap '/bin/rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/old-root" "$tmp/old-control" "$tmp/root/var/lib/dpkg/updates" "$tmp/bin" "$tmp/guard-bin"
touch "$tmp/root/var/lib/dpkg/status"
dpkg-deb --extract "$package" "$tmp/old-root"
dpkg-deb --control "$package" "$tmp/old-control"
version="$(dpkg-deb --field "$package" Version)"
arch="$(dpkg-deb --field "$package" Architecture)"
# A synthetic previous package version with the same installed layout tests
# dpkg's real upgrade transition, not only isolated maintainer-script calls.
sed -i -E 's/^Version: .*/Version: 0.0.1/' "$tmp/old-control/control"
mkdir -p "$tmp/old-root/DEBIAN"
cp -a "$tmp/old-control/." "$tmp/old-root/DEBIAN/"
dpkg-deb --build --root-owner-group "$tmp/old-root" "$tmp/old.deb" >/dev/null
cat > "$tmp/bin/deb-systemd-helper" <<'MOCK'
#!/bin/sh
printf 'helper %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
MOCK
cat > "$tmp/bin/deb-systemd-invoke" <<'MOCK'
#!/bin/sh
printf 'invoke %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
MOCK
# The packaged postrm uses an absolute purge path. Intercept only that exact
# operation so a chrootless dpkg purge cannot touch the runner's host data.
cat > "$tmp/guard-bin/rm" <<'MOCK'
#!/bin/sh
if [ -z "${BLUEROUTE_TEST_ROOT:-}" ] || [ ! -d "$BLUEROUTE_TEST_ROOT/var/lib/dpkg" ]; then
    echo "Missing isolated dpkg root for purge" >&2
    exit 1
fi
if [ "$#" -eq 2 ] && [ "$1" = -rf ] && [ "$2" = /var/lib/blueroute ]; then
    /bin/rm -rf -- "$BLUEROUTE_TEST_ROOT/var/lib/blueroute"
    printf 'purge state\n' >> "$BLUEROUTE_MAINTAINER_LOG"
    exit 0
fi
# dpkg removes its own scratch files during purge. Allow only the isolated
# root, and reject unexpected host paths without invoking the real rm.
if [ "$#" -eq 0 ]; then
    echo "Unexpected empty rm invocation during purge" >&2
    exit 1
fi
for arg in "$@"; do
    case "$arg" in
        -*) ;;
        "$BLUEROUTE_TEST_ROOT"/*) ;;
        *) echo "Unexpected purge cleanup outside isolated root: $arg" >&2; exit 1 ;;
    esac
done
exec /bin/rm "$@"
MOCK
chmod +x "$tmp/bin/"* "$tmp/guard-bin/rm"
export BLUEROUTE_MAINTAINER_LOG="$tmp/service.log"
export BLUEROUTE_TEST_ROOT="$tmp/root"
export PATH="$tmp/bin:$PATH"
# The deletion guard is NOT enabled during installation and upgrade.
# The --root and --script-chrootless flags keep package writes inside tmp,
# while maintainer scripts use the mocked systemd helpers above.
install_pkg() {
    dpkg --root="$tmp/root" --force-not-root --force-script-chrootless \
        --force-depends --install "$1" > "$tmp/dpkg.log" 2>&1 || {
        cat "$tmp/dpkg.log" >&2
        return 1
    }
}
install_pkg "$tmp/old.deb"
grep -Fxq 'invoke start blueroute.service' "$tmp/service.log"
mkdir -p "$tmp/root/var/lib/blueroute"
printf 'node-identity-fixture\n' > "$tmp/root/var/lib/blueroute/node-id"
printf 'network-membership-fixture\n' > "$tmp/root/var/lib/blueroute/membership"
: > "$tmp/service.log"
install_pkg "$package"
test "$(dpkg-query --admindir="$tmp/root/var/lib/dpkg" -W -f='${Version}' blueroute)" = "$version"
grep -Fxq 'invoke restart blueroute.service' "$tmp/service.log"
if grep -Fq 'invoke stop blueroute.service' "$tmp/service.log"; then
    echo "Upgrade unexpectedly stopped the service" >&2
    exit 1
fi
test "$(cat "$tmp/root/var/lib/blueroute/node-id")" = node-identity-fixture
test "$(cat "$tmp/root/var/lib/blueroute/membership")" = network-membership-fixture
test -x "$tmp/root/usr/bin/blueroute"
test -x "$tmp/root/usr/libexec/blueroute/blueroute-daemon"
# Verify the actual dpkg removal transition, including the packaged prerm/postrm.
# Purge is tested below with a guarded rm wrapper to prevent host deletion.
: > "$tmp/service.log"
dpkg --root="$tmp/root" --force-not-root --force-script-chrootless \
    --force-depends --remove blueroute > "$tmp/dpkg.log" 2>&1 || {
    cat "$tmp/dpkg.log" >&2
    exit 1
}
grep -Fxq 'invoke stop blueroute.service' "$tmp/service.log"
grep -Fxq 'helper disable blueroute.service' "$tmp/service.log"
if test -e "$tmp/root/usr/bin/blueroute" ||
   test -e "$tmp/root/usr/libexec/blueroute/blueroute-daemon" ||
   test -e "$tmp/root/usr/lib/systemd/system/blueroute.service" ||
   test -e "$tmp/root/usr/share/dbus-1/system.d/org.blueroute.Service1.conf" ||
   test -e "$tmp/root/usr/share/polkit-1/actions/org.blueroute.policy"; then
    echo "Package removal left package-owned files behind" >&2
    exit 1
fi
test "$(cat "$tmp/root/var/lib/blueroute/node-id")" = node-identity-fixture
test "$(cat "$tmp/root/var/lib/blueroute/membership")" = network-membership-fixture
# Purge is a separate explicit action: unlike remove, it must delete the
# persisted identity and membership. The guarded rm wrapper redirects the
# packaged absolute deletion into the isolated dpkg root.
: > "$tmp/service.log"
PATH="$tmp/guard-bin:$PATH" dpkg --root="$tmp/root" --force-not-root --force-script-chrootless \
    --force-depends --purge blueroute > "$tmp/dpkg.log" 2>&1 || {
    cat "$tmp/dpkg.log" >&2
    exit 1
}
grep -Fxq 'purge state' "$tmp/service.log"
test ! -e "$tmp/root/var/lib/blueroute"
if dpkg-query --admindir="$tmp/root/var/lib/dpkg" -W -f='${Status}' blueroute 2>/dev/null | grep -Fq 'installed'; then
    echo "Purged package still marked installed" >&2
    exit 1
fi
echo "Isolated dpkg upgrade $arch 0.0.1 -> $version preserved state and restarted service; removal retained state; purge deleted state."
