# BlueRoute P14-001 — Formal Security Threat Model

**Status:** Architecture threat assessment, October 13, 2026.
**Applies to:** Linux daemon, system D-Bus client/API, BlueZ PANU/NAP, NetworkManager, persisted node/network trust, future P7 authenticated control sessions, P8/P9 routing, future Internet gateway.
**Source of requirements:** [SPEC.md](SPEC.md) sections 3, 8, 10–17, 20–22 and [TODO.md](TODO.md), particularly P6-004, P7-001..005, P14-001..006, P16 and P17-006.

This document is a **risk analysis**, not a certification or claim that all mitigations are implemented. The present code deliberately refuses production `JoinNetwork` while independent P7 key enrollment and the live authenticated runtime are incomplete. That guard must not be relaxed for software tests or hardware qualification.

## 1. Protected assets and security outcomes

- **Host integrity:** system routing table, forwarding state, NetworkManager profiles, PAN server/client sessions, BlueZ pairing/trust, firewall, DNS, optional future NAT and default route. Never modify or remove non-BlueRoute-owned resources.
- **Network membership and identity:** local long-lived NodeId-bound TLS private key, stable NodeId, explicitly approved per-network peer key pin, network identifier, session transcripts and freshness values. NodeId, source IP, Bluetooth MAC and display name are **not** independent authentication.
- **User data:** ordinary TCP/UDP applications passing over BNEP and routed IP segments. BlueRoute controls admission and routing but cannot promise confidentiality of all user traffic; applications should use SSH/TLS when appropriate.
- **Privilege boundary:** only the system daemon may mutate BlueZ/NetworkManager and Linux routes; desktop/TUI/CLI/browser content must use its versioned D-Bus API and be subject to PolicyKit.
- **Availability and safety:** limited controller connections, CPU, file descriptors, memory, log volume and route convergence; error paths must roll back only owned changes.

**Baseline security goals:** Reject unknown/unapproved peers; prove the peer holds its authorized private key; scope every session and mutation to the intended network; prevent replay and cross-network substitution; avoid unauthenticated route/topology writes; minimize privileges; preserve foreign networking; bound resources; fail closed with actionable non-secret diagnostics.

## 2. Trust boundaries and attacker control

| Boundary | Attacker-controlled inputs | Required decision |
| --- | --- | --- |
| RF/BlueZ scan and PAN discovery | MACs, display names, advertisements, pairing attempts, device properties, link loss and timing | Discovery and BlueZ \`Trusted\` are **hints**, never BlueRoute membership or an authentication proof. |
| PAN IPv4/TCP socket | Source IP/port, TCP connection stream, length prefix and framed message bytes | TCP reachability is not identity. Require mutual TLS 1.3, verified peer public key and approved (NetworkId, NodeId) pin before accepting state. |
| P7 control messages | Claimed sender/network, hello/capability/neighbor facts, session nonce, sequence, version and future route/gateway advertisements | Enforce strict parsing, network/identity/channel binding, fresh nonces, replay guard, message-type authorization and policy validation. |
| System D-Bus | Unprivileged local clients, malformed requests, stale daemon owners and privilege-escalation attempts | Version negotiation, sender-associated PolicyKit checks, typed commands and no direct front-end privileged backend access. |
| Persistence/filesystem | Corrupt/malicious data, symlinks, incompatible schemas, partially written records, privileged or local-account attackers | Restrictive file ownership/modes, no symlink traversal, atomic replacement, strict version/migration and fail-closed parsing. |
| BlueZ/NetworkManager/kernel | Service restarts, unexpected interface renaming, race conditions, foreign routes/profiles and partial mutation | Observe live state, ownership-scoped operations, idempotent rollback, no wildcard cleanup or blind forwarding/default-route changes. |
| Future Internet gateway | Claimed uplink, malicious default route, DNS/NAT/firewall configuration, transit traffic | Opt-in only, explicitly authorized gateway, route-loop protection, firewall ownership and teardown; default-disabled today. |

**Attackers:** (A) nearby unauthenticated Bluetooth device; (B) previously paired or BlueZ-trusted but not an approved BlueRoute member; (C) compromised/hostile legitimate network member; (D) unprivileged local user/process with D-Bus access; (E) malicious or corrupted persistent state; (F) hostile or faulty BlueZ, NetworkManager, or Bluetooth controller; (G) compromised privileged daemon/system user. No software-only defense promises safety against a root-level compromise.

## 3. Threat inventory and concrete controls

| ID / attacker | Abuse and potential impact | Mandatory mitigation | State / residual risk |
| --- | --- | --- | --- |
| T01 / A,B | Spoof advertised network/NodeId or change Bluetooth MAC to trick a join into membership | Distinguish discovery/pairing from authorization; independently user-verify peer key, pin to exact NetworkId and NodeId, require key possession on mutual TLS | BlueZ/advertisement values are treated as non-authoritative; P7 TLS enrollment/UI remains **open**. |
| T02 / A,B | Claim another peer's IP address or use source address on same PAN | Never infer authenticated identity from IP/hostname/PAN interface; bind remote certificate public key and local approval to session | P6-004 correctly fails closed; do not enable based on source IP. |
| T03 / B,C | Replay old hello, session ID or route message to rejoin after revocation or roll back routes | Fresh unpredictable nonces, channel-bound handshakes, monotonic sequence validation, revocation on every newly verified session and authority-sensitive action, bounded session lifetime | Protocol replay/nonce state models exist; **cryptographic TLS binding and live revocation propagation outstanding**. |
| T04 / A,B,C | Malformed, truncated, oversize, flood or unsupported-version messages exhaust memory/CPU or trigger decoder panic | Check 4-byte frame length before allocation, max 16 KiB frame, strict types/schema, bounded feed/session concurrency, timeouts and fuzz/property tests; close poisoned session | Framer/incremental parser tests exist; **live authenticated daemon parser integration and sustained flood tests open**. |
| T05 / B,C | Self-report high capabilities or fictional neighbors and manipulate topology or routable reachability | Treat claims as untrusted until authenticated, then verify policy and reachability; per-node limits, TTL, loop checks, freshness and bounded plan differences | Typed claims are validated; **P8/P9 authorization/reconciliation pending**. Even an authenticated malicious member can lie about link quality. |
| T06 / C | Inject/replace routes, hijack/default-route or transit arbitrary IP through multi-hop | Authorize route type and owner on verified network session; use exact BlueRoute prefixes, prevent route loops, check conflicts and preserve foreign/default routes | Existing single-star owner-scoped route helpers; **full multi-hop authorization pending**. |
| T07 / D | Call daemon mutation methods directly without GUI permission; bypass UI controls or spoof D-Bus sender | System-bus sender identity checked by PolicyKit for every mutation; versioned typed command boundary; least privilege and negative tests | P5-007 service-side PolicyKit tested on Debian; ongoing P14-002 review and new-command audits required. |
| T08 / D | Change trusted peer key silently via legacy NodeId-only approval, revive revoked enrollment | Explicit user-approved key proof, durable (NetworkId, NodeId, SPKI-SHA256) pin; reject missing/mismatched fingerprint; revocation erases pin before trust approval; no implicit rotation | Persistent key pin and daemon check tested; **key enrollment UX, certificate proof, session binding pending**. |
| T09 / D,E | Corrupt state, insert traversal/symlink, race backup/partial writes or downgrade version | Atomic restrictive SecretFileStore and versioned parsers; reject symlinks and future/duplicate records; never default to new identity on corruption | Existing P3 and P7 store tests; concurrent-root attacker and crash consistency still require deployment review. |
| T10 / A,C,D | Hostile display names, diagnostic text or secrets cause UI injection, log injection, command execution or leak | Validate lengths/UTF-8; escape labels in UI and logs; never pass peer fields through shell; redact pins/keys/tokens | Domain validation exists; P14-004/005, Tauri/TUI encoding and support-bundle audit pending. |
| T11 / A,F | PAN, BlueZ or NetworkManager fails during join; orphan interfaces or delete someone else's routes | Transactional reverse cleanup, owned-object tracking, idempotent reconcile from live state and bounded retries; treat external failures as expected | P4/P6 groundwork exists; **P6-004 full joined state and P13 restart/hardware acceptance pending**. |
| T12 / C | Exhaust finite Bluetooth peer slots, continually churn hub, force retry storms | Controller capability-aware ceilings, connection budgets, exponential backoff, anti-flap and explicit rate limits | Session models/backoff exist; live PAN capacity and rate enforcement P6/P9/P13 remain pending. |
| T13 / C | Leak private application traffic or metadata by malicious relay/member even if authenticated | Document network trust scope; use app-layer encryption (SSH/TLS), minimize control metadata and only forward intended prefixes | Membership is **not** end-to-end confidentiality. Do not claim otherwise. |
| T14 / C,G | Later gateway advertises bogus Internet and reroutes/default-DNS traffic, enables NAT without permission | Internet sharing disabled by default; explicit opt-in, authenticated gateway authorization, route/firewall/NAT ownership, gateway revocation and rollback | Gateway implementation postponed to P18, policy must not implicitly accept advertisements. |
| T15 / F,G | Compromised BlueZ/NetworkManager, firmware or privileged daemon controls kernel networking despite BlueRoute checks | Restrict daemon privileges where possible, keep upstream patched, separate capability observation, preserve logs and support matrix | Out-of-scope for protection against root/kernel/firmware compromise; verify deployment hardening independently. |

## 4. Mandatory authenticated-session authorization sequence

Before the daemon may commit remote membership, accept a capability for planning, or process route/topology control:

1. Independently and deliberately **enroll** a peer's public key with an approved (NetworkId, NodeId). Existing NodeId-only P6 trust must **not** authorize this step. Enrollment must authenticate the key out-of-band or via a user-visible, verified ceremony, not TOFU hidden behind pairing.
2. Establish mutually authenticated **TLS 1.3** over a BlueRoute-owned PAN/IP endpoint. Verify both sides' keys and proof of possession with an audited implementation; no plaintext fallback or \`dangerous\` accept-all certificate verifier.
3. Extract the **verified** remote SPKI fingerprint from the peer certificate and check it against current durable network/NodeId pin **and** current approval. Bind the claimed identity and NetworkId to TLS exporter/transcript and fresh local/remote nonces; reject any mismatch.
4. Validate control version, frame bounds, hello/ack challenge echo and immutable negotiated fields. Activate at most one logical session for a peer within capacity limits; enforce monotonic sequence, timeouts and replay guard.
5. Only after all checks succeed may an authenticated *and specifically authorized* message mutate membership or route state. A member's authority is scoped: merely belonging to the network cannot imply permission to advertise default route or modify all topology.
6. On revocation, TLS failure, wrong peer/network, replay, malformed frame, restart or timeout: close session, revoke future control authority, fail closed and reconcile **only BlueRoute-owned** resources. Recheck approval at sensitive decisions.

**Current release gate (reviewed 2026-10-15):** Commit `84f05a6` adds owner-only persistent local TLS identity, exact-leaf-pinned mutual TLS 1.3, peer NodeId/SPKI verification, exporter-bound Hello/HelloAck and durable pin-gated session helpers. Commit `f88e1f3` adds an untrusted candidate-certificate display primitive that only becomes a trust input after matching independent fingerprint confirmation. These pieces are not yet wired to PAN listener/client lifecycle or an independently approved enrollment UI, and route/topology controls remain unimplemented. Steps 1–5 are therefore **not fully wired**. P6-004 production `JoinNetwork` must remain unavailable.

## 5. Verification plan and release gates

- **Unit and negative tests:** missing key pin, wrong network, revoked/stale pin, self-identity claim, wrong TLS key, unsupported protocol/version, duplicate/replayed/out-of-order frames, bogus length fields, malicious UTF-8 and capability/neighbor bounds.
- **Loopback TLS integration (software-only):** independently identified test nodes with distinct keypairs, mutual certificate proof, wrong-key/unknown-key/expired-cert rejection, wrong NetworkId/NodeId, channel-bound hello, parallel session race, timeouts, malformed/corrupt frame, forced restart and teardown. Use real TLS in these tests—plain localhost TCP is insufficient.
- **Privileged boundary tests:** no PolicyKit bypass; denied local mutation, unsupported/malformed command, D-Bus service replacement, no CLI/TUI/Tauri direct system networking; verify redaction and failure classification.
- **Kernel/fake integration:** inject BlueZ/NetworkManager restart, address conflict and partial rollback. Use Linux network namespaces/veth for routing/failover software evidence when runner permits; confirm routes/profiles not owned by BlueRoute are untouched.
- **Physical acceptance deferred:** real Bluetooth PAN pairing/join, spoof attempts, multiple PANU, multi-hop, RF loss, suspend/resume, heterogenous adapters and package lifecycle tracked in [HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md).
- **Prior to P17-006 release:** perform threat-model re-review against actual implementation, implement/qualify all active controls, verify no known authorization bypasses, collect mandatory physical evidence, and publish an accurate support matrix. CI success alone must never close this release gate.

## 6. Explicit assumptions, limitations and outstanding work

**Assumptions:** Linux OS/system D-Bus, BlueZ, NetworkManager and TLS crypto libraries are trusted within their supported boundaries; local root/system compromise is outside BlueRoute's threat containment. BlueRoute does not defeat RF jamming, Bluetooth transport bugs, deliberate malicious traffic from *authorized* members or insecure application protocols.

**Outstanding critical controls:** P7-003 user-verifiable key enrollment UX/ceremony; P7-004 live PAN-bound mutual-TLS session lifecycle and reconnect; P14-003 authorization of future route/topology messages; P6-004 production join sequencing; P13 recovery; P14-002/004/005/006 audit; P18 gateway policies; physical P16 evidence. The TLS helpers authenticate their tested loopback channel, but the control-frame codec by itself is *not* an authenticator and no production peer runtime consumes it yet.

**Maintenance triggers:** Update this model when (a) a new privileged daemon command or third-party client is exposed, (b) peer key enrollment/rotation changes, (c) P8/P9 route or topology messages are added, (d) the gateway is implemented, (e) persistence paths/permissions change, or (f) test evidence contradicts assumed mitigations.

**Acceptance classification:** P14-001 risk *documentation* can be complete while P14-002..006 *implementation and verification* remain open. Documentation does not grant permission to bypass remaining P7/P6/P16 gates.
