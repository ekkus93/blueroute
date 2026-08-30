from pathlib import Path

bluez = Path('crates/blueroute-linux/src/bluez.rs')
text = bluez.read_text()
old = '''    fn request_confirmation(\n        &self,\n        device: OwnedObjectPath,\n        _passkey: u32,\n    ) -> Result<(), PairingAgentError> {\n        self.require_authorized(&device)\n    }'''
new = '''    fn request_confirmation(\n        &self,\n        device: OwnedObjectPath,\n        _passkey: u32,\n    ) -> Result<(), PairingAgentError> {\n        self.require_authorized(&device)?;\n        Err(PairingAgentError::Rejected(\n            "BlueRoute NoInputNoOutput pairing cannot confirm a displayed passkey".to_owned(),\n        ))\n    }'''
if old not in text:
    raise SystemExit('request_confirmation block not found')
text = text.replace(old, new, 1)
old_test = '''        assert!(agent.request_pin_code(path.clone()).is_err());\n        assert!(agent.request_passkey(path).is_err());'''
new_test = '''        assert!(agent.request_pin_code(path.clone()).is_err());\n        assert!(agent.request_passkey(path.clone()).is_err());\n        assert!(agent.request_confirmation(path, 123456).is_err());'''
if old_test not in text:
    raise SystemExit('input rejection test block not found')
text = text.replace(old_test, new_test, 1)
bluez.write_text(text)

doc = Path('docs/P4-004-PAIRING.md')
text = doc.read_text()
text = text.replace(
    'The agent authorizes confirmation, authorization, and service callbacks only for the exact peer whose `BluetoothBackend::pair` operation is active. Unrelated callbacks are rejected. PIN-code and passkey-input requests are rejected because a `NoInputNoOutput` BlueRoute process cannot honestly satisfy them.',
    'The agent authorizes just-works authorization and service callbacks only for the exact peer whose `BluetoothBackend::pair` operation is active. Unrelated callbacks are rejected. PIN-code input, passkey input, and numeric passkey confirmation requests are rejected because a `NoInputNoOutput` BlueRoute process cannot honestly satisfy them.',
)
doc.write_text(text)

todo = Path('docs/TODO.md')
text = todo.read_text()
status_anchor = '| P4-003 | `[x]` | BlueZ discovery lifecycle, Device1 mapping, peer events, and real nearby-Linux-node hardware acceptance are complete; `debiancb1` was observed through the Rust adapter. |\n'
status_line = '| P4-004 | `[-]` | BlueZ pairing, application-agent callbacks, trust/untrust policy, and typed rejection/timeout handling are implemented and green in software tests; two-node physical pairing acceptance remains. |\n'
if status_line not in text:
    if status_anchor not in text:
        raise SystemExit('P4-003 status anchor not found')
    text = text.replace(status_anchor, status_anchor + status_line, 1)
for old, new in [
    ('- [ ] initiate pairing.', '- [x] initiate pairing.'),
    ('- [ ] handle agent/callback needs.', '- [x] handle agent/callback needs.'),
    ('- [ ] trust/untrust according to policy.', '- [x] trust/untrust according to policy.'),
    ('- [ ] typed rejection/timeouts.', '- [x] typed rejection/timeouts.'),
]:
    marker = '## P4-004 — Implement pairing/trust adapter'
    start = text.find(marker)
    if start < 0:
        raise SystemExit('P4-004 section not found')
    end = text.find('\n## P4-005', start)
    if end < 0:
        raise SystemExit('P4-005 section not found')
    section = text[start:end]
    if old in section:
        section = section.replace(old, new, 1)
        text = text[:start] + section + text[end:]
accept = '- Two test nodes complete pairing through Rust-controlled flow.\n'
note = '- Software implementation is complete; physical two-node Rust-controlled pairing evidence is still required before this task becomes `[x]`.\n'
start = text.find('## P4-004 — Implement pairing/trust adapter')
end = text.find('\n## P4-005', start)
section = text[start:end]
if note not in section:
    if accept not in section:
        raise SystemExit('P4-004 acceptance anchor not found')
    section = section.replace(accept, accept + note, 1)
    text = text[:start] + section + text[end:]
todo.write_text(text)
