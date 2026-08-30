from pathlib import Path

path = Path("docs/TODO.md")
text = path.read_text()

replacements = {
    "**Status date:** 2026-08-29": "**Status date:** 2026-08-30",
    "| P4-004 | `[-]` | BlueZ pairing, application-agent callbacks, trust/untrust policy, and typed rejection/timeout handling are implemented and green in software tests; two-node physical pairing acceptance remains. |": "| P4-004 | `[x]` | BlueZ pairing/trust, Rust-controlled Agent1 callbacks, typed rejection/timeout handling, and real two-node Rust-controlled hardware acceptance are complete. |",
    "- Two test nodes complete pairing through Rust-controlled flow.\n- Software implementation is complete; physical two-node Rust-controlled pairing evidence is still required before this task becomes `[x]`.": "- Two test nodes complete pairing through Rust-controlled flow.\n- Hardware acceptance is recorded in `docs/P4-004-HARDWARE-EVIDENCE-2026-08-30.md`; `arisu` paired with `debiancb1` through the Rust-controlled acceptor/initiator flow, and the initiator verified `paired=true` and `trusted=true`.",
}

for old, new in replacements.items():
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"expected exactly one match for {old!r}, found {count}")
    text = text.replace(old, new, 1)

path.write_text(text)
