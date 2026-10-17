#![doc = "Hardware-independent BlueRoute domain logic."]

mod addressing;
mod capability;
mod config;
mod error;
mod forwarding_policy;
mod health;
mod id;
mod membership;
mod neighbor;
mod route;
mod route_planner;
mod secret;
mod segment_allocator;
mod topology;
mod topology_plan;
mod topology_policy;

pub use addressing::{Ipv4StarAddressPlan, ensure_ipv4_segment_available, normalized_ipv4_prefix};
pub use capability::{
    CapabilitySource, LinkQuality, NetworkBackend, NodeCapabilities, PowerState, Sourced,
};
pub use config::{
    BackendPreference, ConfigVersion, DaemonConfig, GatewayPolicy, Ipv4AddressPool, TopologyPolicy,
};
pub use error::{CoreError, ErrorKind};
pub use forwarding_policy::desired_ipv4_forwarding;
pub use health::{HealthComponent, HealthLevel, NodeHealth};
pub use id::{DisplayName, LinkId, NetworkId, NodeId, SegmentId};
pub use membership::{MembershipRegistry, MembershipState, NetworkMembership, PeerMembership};
pub use neighbor::{
    CANDIDATE_LIFETIME_MS, CandidateObservation, CandidateSource, MAX_CANDIDATES,
    NeighborObservations,
};
pub use route::{IpPrefix, Route, RouteDestination, RouteOwner};
pub use route_planner::{DEGRADED_LINK_COST_PENALTY, desired_node_routes};
pub use secret::Secret;
pub use segment_allocator::{MAX_PREFIX_PROBES, MAX_RESERVED_PAN_SEGMENTS, SegmentIpv4Allocator};
pub use topology::{LinkHealth, LinkState, PanLink, PanSegment, Reachability, TopologyGraph};
pub use topology_plan::{NapLink, NapPlanDiff, NapPlanStep, diff_nap_topology};
pub use topology_policy::{
    CandidateLink, NapAssignment, NapSelection, TopologyCandidate, TopologyPolicyInputs,
    select_nap_hubs,
};

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";
