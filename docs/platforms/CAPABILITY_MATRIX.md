# BlueRoute Platform Capability Matrix

This document defines the evidence format for Linux/Bluetooth combinations tested with BlueRoute. It is deliberately hardware-agnostic: vendor/model information identifies a test fixture; it does not define product behavior.

## Capability states

Every capability value should carry a source when the distinction matters:

- **discovered** — reported directly by the running Linux/Bluetooth/network stack;
- **measured** — established by a reproducible physical or integration test;
- **configured** — explicitly selected by administrator or BlueRoute policy;
- **conservative-default** — used when the runtime cannot establish a better value;
- **unknown** — not yet established. Unknown must not be silently converted into an optimistic capability.

A discovered feature can still fail in practice. Measurements therefore override optimistic assumptions when selecting conservative operating policy.

## Required platform identity

Record enough information to reproduce the result:

| Field | Meaning |
| --- | --- |
| report_id | Stable report/evidence identifier |
| date | Test date |
| computer_vendor | Informational vendor |
| computer_model | Informational model |
| architecture | e.g. `x86_64`, `aarch64` |
| distribution | Distribution and release |
| kernel | Exact Linux kernel |
| bluez | Exact BlueZ version |
| network_backend | e.g. NetworkManager and version |
| systemd | Version when relevant |
| bluetooth_controller | Controller/chipset identity |
| bluetooth_bus | USB, PCIe, UART, integrated, other |
| kernel_driver | Bound kernel driver/module |
| firmware | Firmware identifier/version when available |

## Core capability fields

| Field | Type | Meaning |
| --- | --- | --- |
| adapter_usable | yes/no/unknown | Adapter can be used by BlueRoute at all |
| discovery | yes/no/unknown | Nearby-device discovery works |
| pairing | yes/no/unknown | Pair/trust workflow works |
| panu | yes/no/unknown | PANU role is usable |
| nap | yes/no/unknown | NAP role is usable |
| forwarding | yes/no/unknown | Host can route between BlueRoute segments |
| backend | string | Production network backend used for the test |
| practical_peer_limit | integer/unknown | Conservative simultaneously active PAN peer count |
| observed_peer_max | integer/unknown | Largest tested count, not automatically a supported limit |
| suspend_resume | pass/fail/not-applicable/unknown | Recovery behavior on systems with suspend |
| range_recovery | pass/fail/unknown | Recovery after radio loss/return |
| link_quality_signal | string/unknown | Metric available for topology policy, if any |
| power_state_signal | string/unknown | AC/battery information available to policy, if any |
| internet_uplink_detection | yes/no/unknown | Reserved for later gateway work |

Each field may be accompanied by `source`, `evidence`, and `notes`.

## Performance evidence

Performance numbers are test results, not protocol guarantees. Record at least:

- TCP test method, direction, duration, and throughput;
- UDP test method, offered rate, received rate, loss, and jitter where available;
- round-trip latency distribution or at minimum min/average/max;
- sustained-transfer duration;
- number of simultaneous active peers;
- physical distance/radio conditions;
- CPU and memory observations;
- relevant Bluetooth mode/link information exposed by the stack.

Do not compare throughput numbers unless the test methods and topology are sufficiently similar.

## Quirks

Record quirks as structured observations rather than model-specific code requirements. Examples:

- NAP works only after adapter reset;
- connection attempt intermittently leaves a stale BNEP interface;
- controller is stable with three active PAN clients but unstable with four;
- NetworkManager loses profile state after suspend;
- reported RSSI is unavailable while connected.

A quirk should reference evidence and, when code needs a workaround, the workaround should be capability- or behavior-driven rather than keyed only by computer model.

## Example report

```yaml
report_id: example-linux-bt-001
date: 2026-08-29
platform:
  computer_vendor: Example
  computer_model: ExampleBook
  architecture: x86_64
  distribution: Debian 13
  kernel: unknown
  bluez: unknown
  network_backend: NetworkManager
  bluetooth_controller: unknown
capabilities:
  panu:
    value: unknown
    source: unknown
  nap:
    value: unknown
    source: unknown
  practical_peer_limit:
    value: unknown
    source: unknown
```

The example intentionally contains `unknown` values. Unknown evidence is preferable to invented certainty.

## Aggregation rules

A future support matrix may aggregate multiple reports, but it must preserve these rules:

1. One passing system does not establish support for all systems using the same computer model.
2. One controller's peer limit is not a global BlueRoute limit.
3. A maximum observed value is not automatically the recommended policy ceiling.
4. Capability policy should prefer conservative measured evidence when runtime discovery cannot determine a limit safely.
5. Product support labels (`supported`, `tested`, `experimental`, `unsupported`) are release-policy decisions layered on top of evidence, not replacements for the evidence itself.
