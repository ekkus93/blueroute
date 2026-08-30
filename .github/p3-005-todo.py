from pathlib import Path

path = Path("docs/TODO.md")
text = path.read_text()

status_marker = "| P3-004 | `[x]` | Membership persistence uses an explicit v2 schema with tested in-place v1 migration; unsupported old/future schemas fail closed without rewriting state. |\n"
status_row = "| P3-005 | `[x]` | Persistent-secret policy, redacting secret wrapper, and restrictive Linux secret storage are implemented with permission/redaction tests. |\n"
if text.count(status_marker) != 1:
    raise SystemExit("P3-004 status marker not found exactly once")
if status_row not in text:
    text = text.replace(status_marker, status_marker + status_row, 1)

old_task = """## P3-005 — Secure persistent secrets

- [ ] identify secrets.
- [ ] restrictive permissions.
- [ ] log/debug redaction.
"""
new_task = """## P3-005 — Secure persistent secrets

- [x] identify secrets.
- [x] restrictive permissions.
- [x] log/debug redaction.
"""
if text.count(old_task) != 1:
    raise SystemExit("P3-005 task block not found exactly once")
text = text.replace(old_task, new_task, 1)
path.write_text(text)
