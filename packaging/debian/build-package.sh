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
desktop_stage="$(mktemp -d "$output_directory/stage-desktop.XXXXXX")"
trap 'rm -rf "$stage_directory" "$desktop_stage"' EXIT
# mktemp creates 0700 directories; a package's root entry must be 0755.
chmod 0755 "$stage_directory" "$desktop_stage"

cargo build --release --locked -p blueroute-daemon -p blueroute -p blueroute-tui
# The desktop app embeds its built frontend, so build that first (pinned Node, locked deps).
(cd apps/desktop && npm ci && npm run build)
cargo build --release --locked -p blueroute-desktop-app --features custom-protocol

install -D -m 0755 target/release/blueroute-daemon \
  "$stage_directory/usr/libexec/blueroute/blueroute-daemon"
install -D -m 0755 target/release/blueroute \
  "$stage_directory/usr/bin/blueroute"
install -D -m 0755 target/release/blueroute-tui \
  "$stage_directory/usr/bin/blueroute-tui"
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
trap 'rm -rf "$stage_directory" "$desktop_stage" "$verification_directory" "$control_directory"' EXIT
dpkg-deb --extract "$package_path" "$verification_directory"
dpkg-deb --control "$package_path" "$control_directory"

expected_paths=(
  usr/bin/blueroute
  usr/bin/blueroute-tui
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

# blueroute-desktop: the app, its launcher and icon. Kept separate so daemon-only machines do
# not pull in WebKitGTK. Library dependencies are computed from the binary by dpkg-shlibdeps.
install -D -m 0755 target/release/blueroute-desktop-app \
  "$desktop_stage/usr/bin/blueroute-desktop"
install -D -m 0644 packaging/desktop/org.blueroute.desktop \
  "$desktop_stage/usr/share/applications/org.blueroute.desktop"
install -D -m 0644 apps/desktop/src-tauri/icons/128x128.png \
  "$desktop_stage/usr/share/icons/hicolor/128x128/apps/org.blueroute.png"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate packaging/desktop/org.blueroute.desktop
fi
shlibs_root="$(mktemp -d "$output_directory/shlibs.XXXXXX")"
trap 'rm -rf "$stage_directory" "$desktop_stage" "$verification_directory" "$control_directory" "$shlibs_root"' EXIT
mkdir -p "$shlibs_root/debian"
printf 'Source: blueroute\n\nPackage: blueroute-desktop\nArchitecture: any\n' \
  > "$shlibs_root/debian/control"
desktop_shlibs="$(cd "$shlibs_root" && dpkg-shlibdeps -O -e "$desktop_stage/usr/bin/blueroute-desktop" 2>/dev/null)"
desktop_shlibs="${desktop_shlibs#shlibs:Depends=}"
[[ -n "$desktop_shlibs" ]] || {
  echo 'dpkg-shlibdeps found no library dependencies for the desktop app' >&2
  exit 1
}
mkdir -p "$desktop_stage/DEBIAN"
sed \
  -e "s/@VERSION@/$package_version/g" \
  -e "s/@ARCH@/$package_arch/g" \
  -e "s|@SHLIBS@|$desktop_shlibs|g" \
  packaging/debian/control-desktop.in > "$desktop_stage/DEBIAN/control"
chmod 0644 "$desktop_stage/DEBIAN/control"
desktop_package="$output_directory/blueroute-desktop_${package_version}_${package_arch}.deb"
dpkg-deb --build --root-owner-group "$desktop_stage" "$desktop_package"

desktop_verify="$(mktemp -d "$output_directory/verify-desktop.XXXXXX")"
trap 'rm -rf "$stage_directory" "$desktop_stage" "$verification_directory" "$control_directory" "$shlibs_root" "$desktop_verify"' EXIT
dpkg-deb --extract "$desktop_package" "$desktop_verify"
desktop_expected="$(printf '%s\n' \
  usr/bin/blueroute-desktop \
  usr/share/applications/org.blueroute.desktop \
  usr/share/icons/hicolor/128x128/apps/org.blueroute.png | sort)"
desktop_actual="$(find "$desktop_verify" -type f -printf '%P\n' | sort)"
if [[ "$desktop_actual" != "$desktop_expected" ]]; then
  printf 'Desktop package file list differs from the P15-001 manifest.\nExpected:\n%s\nActual:\n%s\n' \
    "$desktop_expected" "$desktop_actual" >&2
  exit 1
fi
desktop_dependencies="$(dpkg-deb --field "$desktop_package" Depends)"
for dependency in "blueroute (= $package_version)" libwebkit2gtk-4.1-0; do
  [[ ", $desktop_dependencies" == *", $dependency"* ]] || {
    printf 'Desktop package is missing dependency: %s\n' "$dependency" >&2
    exit 1
  }
done
# The release binary must embed the frontend just built (a build without custom-protocol
# embeds none and loads the dev server instead). Tauri keys embedded assets by their paths.
for asset in apps/desktop/ui/dist/assets/*; do
  grep -aqF "/assets/$(basename "$asset")" "$desktop_verify/usr/bin/blueroute-desktop" || {
    printf 'Desktop app does not embed %s; build with --features custom-protocol\n' "$asset" >&2
    exit 1
  }
done
for built in "$package_path" "$desktop_package"; do
  root_mode="$(dpkg-deb --contents "$built" | awk '$NF == "./" {print $1; exit}')"
  [[ "$root_mode" == drwxr-xr-x ]] || {
    printf '%s has root directory mode %s, expected drwxr-xr-x\n' "$built" "$root_mode" >&2
    exit 1
  }
done
printf 'Built %s\n' "$desktop_package"
