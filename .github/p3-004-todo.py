from pathlib import Path

path = Path("docs/TODO.md")
text = path.read_text()

status_anchor = '| P3-003 | `[x]` | Durable known-network and peer membership/trust state persists through restart; forget/cleanup and corruption handling are tested and green in CI. |\n'
status_line = '| P3-004 | `[x]` | Membership persistence uses an explicit v2 schema with tested in-place v1 migration; unsupported old/future schemas fail closed without rewriting state. |\n'
if status_line not in text:
    if text.count(status_anchor) != 1:
        raise SystemExit("P3-003 status anchor not found exactly once")
    text = text.replace(status_anchor, status_anchor + status_line, 1)

old = '''## P3-004 — Add schema migration framework

- [ ] version persistent format.
- [ ] migration entry points.
- [ ] synthetic migration test.
'''
new = '''## P3-004 — Add schema migration framework

- [x] version persistent format.
- [x] migration entry points.
- [x] synthetic migration test.
'''
if text.count(old) != 1:
    raise SystemExit("P3-004 checklist marker not found exactly once")
text = text.replace(old, new, 1)

path.write_text(text)
