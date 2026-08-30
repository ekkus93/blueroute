from pathlib import Path

path = Path("docs/TODO.md")
text = path.read_text()

old_status = "| P4-003 | `[-]` | BlueZ StartDiscovery/StopDiscovery, Device1 snapshot mapping, and peer add/change/remove subscriptions are implemented; physical nearby-node acceptance is still pending. |"
new_status = "| P4-003 | `[x]` | BlueZ discovery lifecycle, Device1 mapping, peer events, and real nearby-Linux-node hardware acceptance are complete; `debiancb1` was observed through the Rust adapter. |"

old_acceptance = "- Software implementation is complete; physical nearby-node evidence is still required before this task becomes `[x]`."
new_acceptance = "- Hardware acceptance is recorded in `docs/P4-003-HARDWARE-EVIDENCE-2026-08-29.md`; the known Linux node `debiancb1` appeared through the Rust adapter."

for old, new in ((old_status, new_status), (old_acceptance, new_acceptance)):
    if old not in text:
        raise SystemExit(f"expected TODO text not found: {old}")
    text = text.replace(old, new, 1)

path.write_text(text)
