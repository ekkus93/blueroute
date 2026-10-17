//! Software-only P8-005 projection contract. This test harness compiles
//! the route projection prototype independently while daemon/backend wiring
//! and public Linux adapter integration remain pending.
use blueroute_linux::{LinuxRoute, NetworkInterfaceHandle};

#[path = "../src/route_projection.rs"]
mod route_projection;
