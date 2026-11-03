#!/usr/bin/env bash
# Unprivileged Debian maintainer-script lifecycle contract checks.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
script_dir="${BLUEROUTE_MAINTAINER_SCRIPT_DIR:-$root/packaging/debian}"
for script in postinst prerm postrm; do
    test -f "$script_dir/$script" || { printf "Missing maintainer script: %s\\n" "$script_dir/$script" >&2; exit 1; }
done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
cat > "$tmp/bin/deb-systemd-helper" <<'MOCK'
#!/bin/sh
printf 'helper %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
MOCK
cat > "$tmp/bin/deb-systemd-invoke" <<'MOCK'
#!/bin/sh
printf 'invoke %s\n' "$*" >> "$BLUEROUTE_MAINTAINER_LOG"
MOCK
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
export BLUEROUTE_MAINTAINER_LOG="$tmp/commands"
check() {
    local script="$1" action="$2" expected="$3"
    shift 3
    : > "$BLUEROUTE_MAINTAINER_LOG"
    sh "$script_dir/$script" "$action" "$@"
    local actual
    actual="$(cat "$BLUEROUTE_MAINTAINER_LOG")"
    if [[ "$actual" != "$expected" ]]; then
        printf 'Unexpected %s %s lifecycle calls:\nExpected: %s\nActual: %s\n' "$script" "$action" "$expected" "$actual" >&2
        exit 1
    fi
}
check postinst configure $'helper enable blueroute.service\ninvoke start blueroute.service'
check prerm upgrade ''
check postrm upgrade ''
check postinst configure $'helper enable blueroute.service\ninvoke restart blueroute.service' '0.1.0'
check postinst triggered ''
check prerm remove 'invoke stop blueroute.service'
check prerm deconfigure 'invoke stop blueroute.service'
check postrm remove 'helper disable blueroute.service'
check postrm disappear ''
printf 'Maintainer lifecycle contract passed (fresh install/upgrade/remove).\n'
