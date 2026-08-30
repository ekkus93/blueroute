from pathlib import Path

path = Path("crates/blueroute-linux/src/bluez.rs")
text = path.read_text()
old = "\nstruct PairingPermit {\n"
new = "\n#[derive(Debug)]\nstruct PairingPermit {\n"

if new in text:
    raise SystemExit(0)
if old not in text:
    raise SystemExit("PairingPermit definition not found")

path.write_text(text.replace(old, new, 1))
