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
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/old-root" "$tmp/old-control" "$tmp/root/var/lib/dpkg/updates" "$tmp/bin"
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
chmod +x "$tmp/bin/"*
export BLUEROUTE_MAINTAINER_LOG="$tmp/service.log"
export PATH="$tmp/bin:$PATH"
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
echo "Isolated dpkg upgrade $arch 0.0.1 -> $version preserved state and restarted service."
