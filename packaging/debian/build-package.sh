#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

package_id="$(cargo pkgid -p blueroute-daemon)"
package_version="${package_id##*@}"
package_arch="$(dpkg --print-architecture)"
output_directory="$repo_root/target/debian"
mkdir -p "$output_directory"
stage_directory="$(mktemp -d "$output_directory/stage.XXXXXX")"
trap 'rm -rf "$stage_directory"' EXIT

cargo build --release --locked -p blueroute-daemon -p blueroute

install -D -m 0755 target/release/blueroute-daemon \
  "$stage_directory/usr/libexec/blueroute/blueroute-daemon"
install -D -m 0755 target/release/blueroute \
  "$stage_directory/usr/bin/blueroute"
install -D -m 0644 packaging/systemd/blueroute.service \
  "$stage_directory/usr/lib/systemd/system/blueroute.service"
install -D -m 0644 packaging/dbus/org.blueroute.Service1.conf \
  "$stage_directory/usr/share/dbus-1/system.d/org.blueroute.Service1.conf"
install -D -m 0644 packaging/polkit/org.blueroute.policy \
  "$stage_directory/usr/share/polkit-1/actions/org.blueroute.policy"
install -D -m 0755 packaging/debian/postinst "$stage_directory/DEBIAN/postinst"
install -D -m 0755 packaging/debian/prerm "$stage_directory/DEBIAN/prerm"
install -D -m 0755 packaging/debian/postrm "$stage_directory/DEBIAN/postrm"

mkdir -p "$stage_directory/DEBIAN"
sed \
  -e "s/@VERSION@/$package_version/g" \
  -e "s/@ARCH@/$package_arch/g" \
  packaging/debian/control.in > "$stage_directory/DEBIAN/control"
chmod 0644 "$stage_directory/DEBIAN/control"

package_path="$output_directory/blueroute_${package_version}_${package_arch}.deb"
dpkg-deb --build --root-owner-group "$stage_directory" "$package_path"

verification_directory="$(mktemp -d "$output_directory/verify.XXXXXX")"
control_directory="$(mktemp -d "$output_directory/control.XXXXXX")"
trap 'rm -rf "$stage_directory" "$verification_directory" "$control_directory"' EXIT
dpkg-deb --extract "$package_path" "$verification_directory"
dpkg-deb --control "$package_path" "$control_directory"

expected_paths=(
  usr/bin/blueroute
  usr/libexec/blueroute/blueroute-daemon
  usr/lib/systemd/system/blueroute.service
  usr/share/dbus-1/system.d/org.blueroute.Service1.conf
  usr/share/polkit-1/actions/org.blueroute.policy
)
for relative_path in "${expected_paths[@]}"; do
  test -f "$verification_directory/$relative_path" || {
    printf 'Package is missing manifest path: /%s\n' "$relative_path" >&2
    exit 1
  }
done

for maintainer_script in postinst prerm postrm; do
  test -x "$control_directory/$maintainer_script" || {
    printf 'Package is missing executable maintainer script: %s\n' "$maintainer_script" >&2
    exit 1
  }
done

actual_paths="$(find "$verification_directory" -type f -printf '%P\n' | sort)"
expected_paths_sorted="$(printf '%s\n' "${expected_paths[@]}" | sort)"
if [[ "$actual_paths" != "$expected_paths_sorted" ]]; then
  printf 'Package file list differs from the P15-001 manifest.\nExpected:\n%s\nActual:\n%s\n' \
    "$expected_paths_sorted" "$actual_paths" >&2
  exit 1
fi

package_dependencies="$(dpkg-deb --field "$package_path" Depends)"
for dependency in dbus bluez network-manager polkitd systemd; do
  [[ ", $package_dependencies, " == *", $dependency,"* ]] || {
    printf 'Package is missing runtime dependency: %s\n' "$dependency" >&2
    exit 1
  }
done
printf 'Built %s\n' "$package_path"
