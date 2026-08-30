from pathlib import Path

path = Path("docs/TODO.md")
text = path.read_text()

old_table = "| P4-001 | `[x]` | Backend-neutral adapter boundaries and fake-backend tests are implemented and green in CI. |\n| P5-001 | `[-]` |"
new_table = "| P4-001 | `[x]` | Backend-neutral adapter boundaries and fake-backend tests are implemented and green in CI. |\n| P4-002 | `[x]` | Direct system-D-Bus BlueZ service/adapter discovery, Powered-state mapping, and adapter-change subscriptions are implemented and green in locked CI; physical-controller validation remains in P1. |\n| P5-001 | `[-]` |"
if old_table not in text:
    raise SystemExit("P4 status-table anchor not found")
text = text.replace(old_table, new_table, 1)

old_task = '''## P4-002 — Implement BlueZ service/adapter discovery

- [ ] Rust system-D-Bus connection.
- [ ] enumerate adapters.
- [ ] observe power state.
- [ ] subscribe to changes.
'''
new_task = '''## P4-002 — Implement BlueZ service/adapter discovery

- [x] Rust system-D-Bus connection.
- [x] enumerate adapters.
- [x] observe power state.
- [x] subscribe to changes.
'''
if old_task not in text:
    raise SystemExit("P4-002 checklist anchor not found")
text = text.replace(old_task, new_task, 1)
path.write_text(text)
