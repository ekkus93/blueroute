# Module Split Evidence

Versioned evidence for [MODULE_SPLIT_TODO.md](MODULE_SPLIT_TODO.md) (specification:
[MODULE_SPLIT_SPEC.md](MODULE_SPLIT_SPEC.md)). Software-only refactor; no product claim changes.

## MS-BASE — Baseline (2026-11-16)

**Environment (MS-BASE-01).** Source `48f5dda` (`1870e40` plus the two plan documents; no code
difference), clean tracked tree apart from `.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, Debian
13 kernel 6.12.115.

**Files to split** (production lines precede the file's `#[cfg(test)]` module):

| File | Total lines | Production | Tests in file |
| --- | ---: | ---: | ---: |
| `apps/cli/src/main.rs` | 2,090 | 1,167 | 22 |
| `crates/blueroute-linux/src/networkmanager.rs` | 2,107 | 1,787 | 14 |
| `apps/daemon/src/control_tls.rs` | 2,907 | 1,598 | 22 |
| `crates/blueroute-linux/src/bluez.rs` | 1,643 | 1,348 | 15 |

**Baseline snapshots (MS-BASE-02/03)**, taken twice with identical results:

| Snapshot | Baseline |
| --- | --- |
| `cargo test -p blueroute --bins -- --list` | 22 tests |
| `cargo test -p blueroute-linux --lib -- --list` | 153 tests (14 `networkmanager::`, 15 `bluez::`) |
| `cargo test -p blueroute-daemon --lib -- --list` | 343 tests (22 `control_tls::`) |
| CLI behavior matrix | 62 cases (31 argv, each with and without `--json`): exit 0 ×5, 2 ×20, 3 ×37 |
| `blueroute_linux` public doc items (`all.html` links) | 81 |
| `blueroute_daemon` public doc items | 155 |

The CLI prints one top-level help screen for every help request (`parse` maps clap's
`DisplayHelp` to `Operation::Help`), so the matrix covers help through `--help` and `help network`
rather than per-subcommand help. With the system bus pointed at a missing socket, commands that
need the daemon fail with exit 3 and the daemon-unavailable guidance before sending anything.

**Comparison method (MS-BASE-04).** The capture script below is run as `capture.sh base` before
and `capture.sh <task>` after each split; a split passes SPLIT-2/4 when the leaf test-name
multisets (`leaf-*`), the CLI case files (`cli/`) and the doc item lists (`doc-*`) are identical
to the baseline. Full test paths may differ only by module path. The script (kept here so the
comparison does not depend on ignored files):

```bash
#!/usr/bin/env bash
# MS capture: test-name listings, CLI help text and public doc items, into tmp/ms/<label>/.
set -uo pipefail
repo=/home/phil/work/blueroute; cd "$repo" || exit 1
export TMPDIR=$repo/tmp
out=$repo/tmp/ms/${1:?label}; rm -rf "$out"; mkdir -p "$out"
git rev-parse HEAD > "$out/head"; git status --short > "$out/status"
list() { cargo test --locked "$@" -- --list 2>/dev/null | sed -n 's/: test$//p' | sort; }
list -p blueroute --bins > "$out/tests-cli"
list -p blueroute-linux --lib > "$out/tests-linux"
list -p blueroute-daemon --lib > "$out/tests-daemon"
leaf() { sed 's/.*:://' "$1" | sort; }
for s in cli linux daemon; do leaf "$out/tests-$s" > "$out/leaf-$s"; done
cargo build --locked -p blueroute -q 2>/dev/null
bin=$repo/target/debug/blueroute
# CLI behavior with no reachable daemon: the system bus address names a socket that does not
# exist, so every command fails before anything is sent. stdout, stderr and exit status per argv.
node=00112233445566778899aabbccddeeff
fp=0000000000000000000000000000000000000000000000000000000000000000
mkdir -p "$out/cli"
i=0
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    i=$((i+1)); read -ra argv <<< "$line"
    for json in "" "--json"; do
        f=$(printf '%s/cli/%03d%s' "$out" "$i" "${json:+-json}")
        { echo "argv: $line $json"
          DBUS_SYSTEM_BUS_ADDRESS="unix:path=$repo/tmp/ms/no-such-bus" "$bin" "${argv[@]}" $json \
              > "$f.out" 2> "$f.err"; echo "exit: $?"; } > "$f.meta"
    done
done <<LIST
--help
--version
help network
status
capability show
diagnostics show
diagnostics show --summary
network list
network create Home
network create
network join $node
network leave
network link
network reconnect
node list
node show $node
node show not-a-node
node set-name Laptop
peer local-key
peer verify-local-key $node $fp
peer verify-local-key $node $fp --network $node
peer verify-local-key $node short
peer trust $node
peer enroll $node $fp
peer forget $node
peer set-join-host $node $node $fp
peer clear-join-host
discover start
discover stop
no-such-command
network
LIST
cargo doc --no-deps --locked -q -p blueroute-linux -p blueroute-daemon 2>/dev/null
for c in blueroute_linux blueroute_daemon; do
    grep -o 'href="[^"]*"' "$repo/target/doc/$c/all.html" | sort > "$out/doc-$c"
done
{ for s in cli linux daemon; do echo "$s $(wc -l < "$out/tests-$s")"; done
  echo "networkmanager $(grep -c '^networkmanager::' "$out/tests-linux")"
  echo "bluez $(grep -c '^bluez::' "$out/tests-linux")"
  echo "control_tls $(grep -c '^control_tls::' "$out/tests-daemon")"
  echo "cli-cases $(ls "$out/cli"/*.meta | wc -l)"
  for c in blueroute_linux blueroute_daemon; do echo "doc-$c $(wc -l < "$out/doc-$c")"; done; } | tee "$out/summary"
```
