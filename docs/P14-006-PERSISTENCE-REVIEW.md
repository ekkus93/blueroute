# P14-006 — Persistence security review

**Reviewed:** 2026-10-15

## Storage boundaries

The packaged daemon runs as root with systemd-managed `/var/lib/blueroute` (`StateDirectory=blueroute`, mode `0700`) and `UMask=0077`; see [P5-006-SYSTEMD.md](P5-006-SYSTEMD.md). Stable identity and membership files are stored directly under this protected directory. TLS identities and peer-key pins use dedicated owner-only subdirectories. The forwarding lease is boot-local under `/run/blueroute`.

The file stores enforce restrictive file/directory modes and reject a symlink or other non-regular object at the final file path before reading or replacing it. Protection of parent path components relies on the systemd-owned state/runtime directories and the local-root trust boundary. A manually supplied `BLUEROUTE_STATE_DIR` is an administrator/development override and must itself be protected from untrusted writers.

| State | Protection and recovery behavior |
| --- | --- |
| Node identity | `create_new`, mode `0600`, file sync and parent-directory sync on first creation; final symlinks, non-regular files and malformed identities fail closed without identity rotation. |
| Membership registry | Mode `0600`, versioned strict parsing, validated v1-to-v2 migration, same-directory temp write, file sync, atomic rename and directory sync. Malformed, unsupported-future, symlink and non-regular state is rejected rather than reset. |
| TLS identity and other secret bytes | `SecretFileStore` uses mode `0700` directories and mode `0600` files, validates single-component names, rejects final-path symlinks, syncs writes and directories, and supports atomic replacement or hard-link-based first-writer-wins creation. Secret `Debug` output is redacted. |
| Peer-key pins | Secret storage protections plus bounded strict records, zero-identity/duplicate/alias rejection, corruption failure, atomic replacement, and a persistent owner-only process lock for cooperating daemon authorization changes. Revocation removes the pin before membership trust state. |
| Forwarding lease | Boot-local owner-only lease records the baseline before enabling the node-global sysctl. Existing lease paths must be regular files and are tightened to mode `0600`; symlinks and malformed data fail closed. Release without a BlueRoute lease leaves foreign state alone. |

## Evidence

Store-level tests cover owner-only modes and permission tightening, restart round trips, malformed/corrupt/future-schema rejection, symlink targets left untouched, atomic replacement without temporary-file residue, concurrent first-secret creation, and revocation/reapproval behavior. Node identity, membership, secret, peer-pin and forwarding-lease paths have explicit symlink rejection coverage. Node identity first creation also syncs its parent directory after syncing the file. The Linux crate suite passed after these changes; no CI or physical-hardware claim is made for this local work.

## Limits

These controls protect against unprivileged access under the packaged systemd directory ownership model; they do not protect state from root or another process with equivalent privileges. The process lock coordinates cooperating BlueRoute daemons only. It does not make arbitrary privileged external file edits safe, nor does it replace backup, migration, or recovery policy.
