# P6-002 — Discoverable BlueRoute network identity

## Scope

P6-002 makes one hosted single-star BlueRoute network discoverable by a second compatible Linux node without requiring a manually entered Bluetooth MAC address.

This task owns:

- over-the-air advertisement of the hosted `NetworkId`;
- recognition of nearby BlueRoute NAP candidates from BlueZ `Device1` advertisement metadata;
- daemon `StartDiscovery`, `StopDiscovery`, and live `ListNetworks` behavior;
- deterministic merging of remembered and nearby network identities;
- explicit separation between discovery metadata and security identity.

It does **not** grant membership, pair/trust a peer, establish PANU, allocate a client address, or authenticate the advertised identity. Those remain P6-003, P6-004, P6-005, and the P7 authenticated control-plane work.

## Advertisement format

A hosted network registers an `org.bluez.LEAdvertisement1` object through the adapter's `org.bluez.LEAdvertisingManager1`. BlueRoute requests a discoverable peripheral advertisement and places one compact record in `ManufacturerData`.

P6-002 uses manufacturer key `0xffff` as an application-private development sentinel. It does not claim a Bluetooth SIG company identifier. The value is exactly 20 bytes:

| Offset | Size | Meaning |
| --- | ---: | --- |
| `0` | 2 | ASCII magic `BR` |
| `2` | 1 | advertisement schema version, currently `1` |
| `3` | 1 | role flags; bit `0` means hosted NAP candidate |
| `4` | 16 | full BlueRoute `NetworkId` bytes |

The full 128-bit logical network identity therefore survives discovery unchanged. The Bluetooth address is only the BlueZ transport handle used to reach the advertising device; it is not the BlueRoute network identity.

The payload deliberately omits the user-editable network/display name. A nearby-only `NetworkSummary` receives a deterministic presentation label of `BlueRoute <first-8-network-id-hex>`. This keeps the stable identity independent of Bluetooth `Name`/`Alias` and avoids depending on truncated local-name fields in legacy LE advertisement space.

## Host lifecycle

`CreateNetwork` now treats discovery advertisement as required hosted-star runtime state:

1. create/reconcile bridge;
2. assign host address;
3. register BlueZ NAP;
4. register the BlueRoute LE advertisement;
5. only then allow durable membership commit.

If advertisement registration fails, NAP/address/bridge state is rolled back and membership is not committed. Runtime teardown unregisters the advertisement before stopping NAP and removing network state. Cleanup failures are surfaced rather than silently discarded.

The adapter must expose `org.bluez.LEAdvertisingManager1`. Missing advertising support, disabled adapters, advertisement-length rejection, and exhausted/not-permitted advertising capacity return typed capability/state errors; no adapter/computer model allowlist is used.

## Discovery lifecycle

`StartDiscovery` and `StopDiscovery` are implemented by the daemon through the existing direct BlueZ adapter.

- Scanning is explicit and bounded by the caller; BlueRoute does not continuously scan in the background.
- Repeated `StartDiscovery` for BlueRoute's own active session is idempotent.
- Repeated `StopDiscovery` after the session is already stopped is idempotent.
- BlueZ failures are surfaced; an internal state-install failure after `StartDiscovery` attempts to stop the just-started session and reports rollback failure if that cleanup also fails.
- While the session is active, `ListNetworks` takes a fresh BlueZ peer snapshot and accepts only devices carrying a valid P6-002 advertisement record.
- Nearby candidates are keyed by `NetworkId`, not by MAC address or Bluetooth/display name.
- Durable remembered membership wins if a nearby device advertises the same `NetworkId`, so spoofable discovery presentation data cannot overwrite local durable metadata.

The reusable client example `network_discovery_probe` performs a ten-second discovery window and prints the candidate network IDs returned by the daemon:

```bash
cargo run -p blueroute-client --example network_discovery_probe --locked
```

No Bluetooth address is an input to that probe.

## Security boundary

P6-002 advertisement data is **unauthenticated discovery metadata**.

An attacker in radio range can copy, spoof, replay, or suppress the advertisement, including a valid-looking `NetworkId`. The advertised identifier is not a secret and is not proof that the broadcaster owns or is authorized for that BlueRoute network. Bluetooth `Name`, `Alias`, MAC/address metadata, RSSI, and pairing status likewise are not BlueRoute authorization evidence.

Consequently:

- discovery may create a *candidate* network only;
- P6-003 must still require intended pairing/membership approval;
- later P7 control-plane authentication must bind the communicating peer to BlueRoute identity rather than trusting source IP or discovery metadata;
- a spoofed advertisement must never by itself grant membership or cause privileged network mutation.

## Known limitations

1. **Stable-ID privacy:** advertising the stable `NetworkId` permits passive correlation/tracking of a hosted network. A future production enrollment design may replace the stable public token with rotating/invitation-derived discovery material while preserving authenticated logical identity after contact.
2. **Development manufacturer key:** `0xffff` is used only as a private prototype sentinel and is not a registered Bluetooth SIG company identifier. A production format needs an appropriate assigned or standards-compatible advertising namespace.
3. **LE advertising dependency:** a NAP-capable controller that lacks BlueZ LE advertising support cannot satisfy P6-002 and hosting now fails closed rather than creating an undiscoverable star.
4. **Snapshot API:** P6-002 exposes nearby candidates through `ListNetworks` while discovery is active. The existing `NetworkDiscovered`/`NetworkLost` event types are not yet driven by a long-lived discovery worker.
5. **Service/adapter resets:** BlueZ/controller loss can invalidate active scan or advertisement state. Full runtime re-observation/reconciliation remains P6-009/P6-010 reliability work.
6. **Presentation name:** the original user-edited network name is not carried in the compact advertisement. Nearby candidates use the deterministic short-ID label until a later authenticated exchange can provide richer presentation metadata.

## Acceptance

Deterministic tests cover payload round-trip, malformed/unknown advertisement rejection, independence from Bluetooth names, remembered-network precedence, and authorized daemon dispatch.

Physical acceptance still requires a second compatible Linux node to run the production daemon/client discovery path and observe the hosted network's exact `NetworkId` without providing a Bluetooth MAC address. Hardware evidence is recorded separately before P6-002 is closed.
