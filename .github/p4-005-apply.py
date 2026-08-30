from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old!r}")
    file.write_text(text.replace(old, new, 1))


replace_once(
    "crates/blueroute-linux/src/bluez.rs",
    "pub struct BluezBackend {\n    connection: Connection,\n    pairing: Arc<PairingControl>,\n}",
    "pub struct BluezBackend {\n    pub(crate) connection: Connection,\n    pairing: Arc<PairingControl>,\n}",
)

replace_once(
    "crates/blueroute-linux/src/lib.rs",
    "mod bluez;\nmod identity;",
    "mod bluez;\nmod identity;\nmod pan;",
)

replace_once(
    "crates/blueroute-linux/src/lib.rs",
    "#[derive(Clone, Debug, Eq, PartialEq)]\npub struct PanAttachment {\n    pub role: PanRole,\n    pub interface: NetworkInterfaceHandle,\n    pub peer: Option<PeerHandle>,\n}\n\n/// PAN lifecycle boundary. Its implementation may ultimately use BlueZ, NetworkManager, or both.\npub trait PanBackend: Send + Sync {\n    fn connect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, PanAttachment>;\n    fn disconnect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, ()>;\n    fn start_nap(&self, adapter: AdapterHandle) -> BackendFuture<'_, PanAttachment>;\n    fn stop_nap(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;\n}",
    "#[derive(Clone, Debug, Eq, PartialEq)]\npub struct PanAttachment {\n    pub role: PanRole,\n    pub interface: NetworkInterfaceHandle,\n    pub peer: Option<PeerHandle>,\n}\n\n#[derive(Clone, Debug, Eq, PartialEq)]\npub enum PanuEvent {\n    Lost(PanAttachment),\n}\n\n/// Pull-based PANU link event subscription independent of BlueZ/D-Bus stream types.\npub trait PanuEventSubscription: Send {\n    fn next_event(&mut self) -> BackendFuture<'_, Option<PanuEvent>>;\n}\n\n/// PAN lifecycle boundary. Its implementation may ultimately use BlueZ, NetworkManager, or both.\npub trait PanBackend: Send + Sync {\n    fn connect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, PanAttachment>;\n    fn disconnect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, ()>;\n    fn subscribe_panu_events(\n        &self,\n        attachment: PanAttachment,\n    ) -> BackendFuture<'_, Box<dyn PanuEventSubscription>>;\n    fn start_nap(&self, adapter: AdapterHandle) -> BackendFuture<'_, PanAttachment>;\n    fn stop_nap(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;\n}",
)

replace_once(
    "crates/blueroute-linux/README.md",
    "Device discovery uses `org.bluez.Adapter1.StartDiscovery` and `StopDiscovery`, maps `org.bluez.Device1` objects into D-Bus-neutral `DiscoveredPeer` values, and exposes peer add/change/remove events without maintaining an unbounded local peer cache. Pairing/trust and Bluetooth PAN lifecycle remain separate follow-on tasks.",
    "Device discovery uses `org.bluez.Adapter1.StartDiscovery` and `StopDiscovery`, maps `org.bluez.Device1` objects into D-Bus-neutral `DiscoveredPeer` values, and exposes peer add/change/remove events without maintaining an unbounded local peer cache. Pairing/trust uses a Rust-controlled BlueZ `Agent1` flow. PANU lifecycle uses `org.bluez.Network1.Connect(\"nap\")`/`Disconnect()`, returns the BlueZ-created BNEP interface through `PanAttachment`, and exposes bounded link-loss events. NAP lifecycle and IP configuration remain separate follow-on tasks.",
)

replace_once(
    "docs/TODO.md",
    "| P4-004 | `[x]` | BlueZ pairing/trust, Rust-controlled Agent1 callbacks, typed rejection/timeout handling, and real two-node Rust-controlled hardware acceptance are complete. |\n| P5-001 | `[-]` | API version contract and compatibility rules are implemented/tested; client-side incompatibility enforcement remains pending P5-005. |",
    "| P4-004 | `[x]` | BlueZ pairing/trust, Rust-controlled Agent1 callbacks, typed rejection/timeout handling, and real two-node Rust-controlled hardware acceptance are complete. |\n| P4-005 | `[-]` | BlueZ Network1 PANU connect/interface mapping, loss observation, and idempotent disconnect are implemented; working PANU data-plane hardware acceptance remains pending. |\n| P5-001 | `[-]` | API version contract and compatibility rules are implemented/tested; client-side incompatibility enforcement remains pending P5-005. |",
)

replace_once(
    "docs/TODO.md",
    "## P4-005 — Implement PANU connection adapter\n\n- [ ] establish PANU connection through selected API.\n- [ ] identify resulting interface.\n- [ ] observe loss.\n- [ ] idempotent disconnect.\n\n**Acceptance**\n\n- Hardware integration path creates a working PANU data plane on supported test hardware.",
    "## P4-005 — Implement PANU connection adapter\n\n- [x] establish PANU connection through selected API.\n- [x] identify resulting interface.\n- [x] observe loss.\n- [x] idempotent disconnect.\n\n**Acceptance**\n\n- Hardware integration path creates a working PANU data plane on supported test hardware.\n- Software implementation is complete; real PANU data-plane evidence is still required before this task becomes `[x]`.\n- Implementation/design notes and the hardware probe are documented in `docs/P4-005-PANU.md`.",
)
