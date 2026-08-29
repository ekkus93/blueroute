#![doc = "Hardware-independent BlueRoute domain logic."]

mod capability;
mod config;
mod error;
mod health;
mod id;
mod membership;
mod route;
mod topology;

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
pub use route::{IpPrefix, Route, RouteDestination, RouteOwner};
pub use topology::{LinkHealth, LinkState, PanLink, PanSegment, Reachability, TopologyGraph};

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";
