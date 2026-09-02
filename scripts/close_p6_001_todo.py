# Temporary one-shot helper; removed before merge.
# This follow-up commit intentionally triggers the PR workflow after branch checkout was pinned.
from pathlib import Path

path = Path("docs/TODO.md")
text = path.read_text()

status_anchor = "| P5-007 | `[x]` | Least-privilege D-Bus/PolicyKit authorization is implemented and live Debian acceptance proves unprivileged read-only inspection plus fail-closed unauthorized mutation denial. |\n| All other tasks | `[ ]` | Not started. |"
status_replacement = "| P5-007 | `[x]` | Least-privilege D-Bus/PolicyKit authorization is implemented and live Debian acceptance proves unprivileged read-only inspection plus fail-closed unauthorized mutation denial. |\n| P6-001 | `[x]` | CreateNetwork establishes durable member state, a deterministic BlueRoute-owned NetworkManager bridge/address, a live BlueZ NAP, and real PANU attachment on physical hardware. |\n| All other tasks | `[ ]` | Not started. |"

if status_anchor in text:
    text = text.replace(status_anchor, status_replacement, 1)
elif "| P6-001 | `[x]` |" not in text:
    raise SystemExit("P6-001 status-table anchor not found")

old = """## P6-001 — Implement create-network operation

- [ ] create logical network ID/name.
- [ ] persist membership.
- [ ] select local NAP only if capability permits.
- [ ] establish local subnet.

**Acceptance**

- `CreateNetwork` yields stable daemon state on a NAP-capable Linux node.
- Unsupported NAP capability produces a clear error rather than a model-name special case.
"""

new = """## P6-001 — Implement create-network operation

- [x] create logical network ID/name.
- [x] persist membership.
- [x] select local NAP only if capability permits.
- [x] establish local subnet.

**Acceptance**

- `CreateNetwork` yields stable daemon state on a NAP-capable Linux node.
- Unsupported NAP capability produces a clear error rather than a model-name special case.
- Hardware acceptance is recorded in `docs/P6-001-HARDWARE-EVIDENCE-2026-09-01.md`; the production daemon created network `26ed3f29d622ae9c5c68635f4d548bbe`, persisted stable member state, created `brb-26ed3f29` with `10.201.41.1/24`, registered a live NAP, and accepted `arisu` so the server-side Bluetooth interface `enxf4d10870b786` became a kernel member of that exact bridge.
"""

if old in text:
    text = text.replace(old, new, 1)
elif "- [x] create logical network ID/name." not in text:
    raise SystemExit("P6-001 task block anchor not found")

path.write_text(text)
