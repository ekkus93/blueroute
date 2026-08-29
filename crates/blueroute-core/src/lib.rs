#![doc = "Hardware-independent BlueRoute domain logic."]

mod capability;
mod error;
mod health;
mod id;
mod membership;
mod route;
mod topology;

pub use capability::{
    CapabilitySource, LinkQuality, NetworkBackend, NodeCapabilities, PowerState, Sourced,
};
pub use error::{CoreError, ErrorKind};
pub use health::{HealthComponent, HealthLevel, NodeHealth};
pub use id::{DisplayName, LinkId, NetworkId, NodeId, SegmentId};
pub use membership::{MembershipState, NetworkMembership};
pub use route::{IpPrefix, Route, RouteDestination, RouteOwner};
pub use topology::{LinkHealth, LinkState, PanLink, PanSegment, TopologyGraph};

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";
