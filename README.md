# BlueRoute

BlueRoute is a friendly Linux application for creating ordinary TCP/IP networks over Bluetooth PAN/BNEP without requiring Wi-Fi infrastructure.

The project is designed around a shared Rust core and background daemon, with CLI, TUI, and Tauri desktop front ends. Multi-hop networking is implemented with normal Linux IP routing between Bluetooth PAN segments rather than by tunneling IP through Bluetooth Mesh.

BlueRoute targets compatible Linux computers with supported Bluetooth PAN hardware. Dell Chromebook 3100 systems are useful development/test fixtures, not a product requirement.

## Project status

BlueRoute is in early development. The architecture and implementation backlog are maintained in:

- `docs/SPEC.md`
- `docs/TODO.md`

See `CONTRIBUTING.md` and `docs/DEVELOPMENT.md` for development conventions and checks.
