# P7-004 peer-scoped callback qualification

On 2026-10-16, peer-scoped cancellation was implemented in `RegisteredControlSession`. A callback must match both the network/peer key and the TLS session generation. Tests reject wrong-network, wrong-peer, and stale-generation callbacks. Exact-head CI 38962391093 passed at 7c9c9877b598bfd7c22d369a880f0423ce320992.

This is a software-only guard. Daemon PAN callback wiring, independent key enrollment, production JoinNetwork, and physical Bluetooth acceptance remain outstanding.
