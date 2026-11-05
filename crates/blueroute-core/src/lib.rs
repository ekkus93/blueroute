#![doc = "Hardware-independent BlueRoute domain logic."]

mod addressing;
mod capability;
mod config;
mod error;
mod health;
mod id;
mod membership;
mod neighbor;
mod route;
mod routing;
mod secret;
mod subnet;
mod topology;

pub use addressing::{Ipv4StarAddressPlan, ensure_ipv4_segment_available, normalized_ipv4_prefix};
pub use capability::{
    CapabilitySource, LinkQuality, NetworkBackend, NodeCapabilities, PowerState, Sourced,
};
pub use config::{
    BackendPreference, ConfigVersion, DaemonConfig, GatewayPolicy, Ipv4AddressPool, TopologyPolicy,
};
pub use error::{CoreError, ErrorKind};
pub use health::{HealthComponent, HealthLevel, NodeHealth};
pub use id::{DisplayName, LinkId, NetworkId, NodeId, SegmentId};
pub use membership::{MembershipRegistry, MembershipState, NetworkMembership, PeerMembership};
pub use neighbor::{Neighbor, NeighborLink, NeighborTable};
pub use route::{IpPrefix, Route, RouteDestination, RouteOwner};
pub use routing::{ComputedRoutes, compute_routes};
pub use secret::Secret;
pub use subnet::SegmentSubnets;
pub use topology::{LinkHealth, LinkState, PanLink, PanSegment, Reachability, TopologyGraph};

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";
