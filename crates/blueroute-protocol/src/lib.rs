#![doc = "Shared BlueRoute protocol and API types."]

use std::fmt;

use blueroute_core::{
    DisplayName, HealthLevel, LinkHealth, LinkId, LinkState, MembershipState, NetworkId,
    NodeCapabilities, NodeId, Reachability, Route,
};

/// Version of the local daemon API contract.
///
/// Major versions are incompatible. Minor versions are additive: a server can
/// serve a client with the same major version and a minor version no newer than
/// the server's own minor version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    pub const CURRENT: Self = Self { major: 1, minor: 0 };

    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    pub const fn compatibility_with_server(self, server: Self) -> ApiCompatibility {
        if self.major != server.major || self.minor > server.minor {
            ApiCompatibility::Incompatible
        } else if self.minor == server.minor {
            ApiCompatibility::Exact
        } else {
            ApiCompatibility::Compatible
        }
    }

    pub const fn is_compatible_with_server(self, server: Self) -> bool {
        !matches!(
            self.compatibility_with_server(server),
            ApiCompatibility::Incompatible
        )
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiCompatibility {
    Exact,
    Compatible,
    Incompatible,
}

/// Well-known system D-Bus service name for the v1 daemon API.
pub const DBUS_SERVICE_NAME: &str = "org.blueroute.Service1";
/// Well-known root object path for the v1 daemon API.
pub const DBUS_OBJECT_PATH: &str = "/org/blueroute/Service1";
/// Well-known D-Bus interface for the v1 daemon API.
pub const DBUS_INTERFACE_NAME: &str = "org.blueroute.Service1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkSummary {
    pub id: NetworkId,
    pub name: DisplayName,
    pub member_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeSummary {
    pub id: NodeId,
    pub name: DisplayName,
    pub reachability: Reachability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonStatus {
    pub api_version: ApiVersion,
    pub local_node: Option<NodeId>,
    pub current_network: Option<NetworkId>,
    pub health: HealthLevel,
    pub capabilities: NodeCapabilities,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticSnapshot {
    pub api_version: ApiVersion,
    pub health: HealthLevel,
    pub current_network: Option<NetworkId>,
    pub visible_nodes: u32,
    pub capabilities: NodeCapabilities,
}

/// Semantic local-daemon operations shared by all front ends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    GetStatus,
    GetCapabilities,
    ListNetworks,
    CreateNetwork {
        name: DisplayName,
    },
    JoinNetwork {
        network: NetworkId,
    },
    LeaveNetwork,
    ListNodes,
    GetNode {
        node: NodeId,
    },
    SetDeviceName {
        name: DisplayName,
    },
    StartDiscovery,
    StopDiscovery,
    TrustPeer {
        node: NodeId,
    },
    ForgetPeer {
        node: NodeId,
    },
    GetDiagnostics,
    /// Reserved for the future gateway phase. Current daemons must reject it.
    SetInternetSharing {
        enabled: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Response {
    Ack,
    Status(DaemonStatus),
    Capabilities(NodeCapabilities),
    Networks(Vec<NetworkSummary>),
    Nodes(Vec<NodeSummary>),
    Node(Option<NodeSummary>),
    Diagnostics(DiagnosticSnapshot),
}

/// State-change events published by the daemon to local clients.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    NetworkDiscovered(NetworkSummary),
    NetworkLost(NetworkId),
    NodeChanged(NodeSummary),
    NodeDisconnected(NodeId),
    CapabilitiesChanged {
        node: NodeId,
        capabilities: NodeCapabilities,
    },
    MembershipChanged {
        network: NetworkId,
        state: MembershipState,
    },
    LinkChanged {
        link: LinkId,
        state: LinkState,
        health: LinkHealth,
    },
    TopologyChanged {
        network: NetworkId,
    },
    RouteChanged(Route),
    HealthChanged(HealthLevel),
    AuthorizationFailed {
        operation: String,
    },
    InternetAvailabilityChanged {
        available: bool,
    },
    /// Reserved for the future gateway phase.
    GatewayAvailabilityChanged {
        node: NodeId,
        available: bool,
    },
}

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use blueroute_core::{
        CapabilitySource, NetworkId, NodeId, RouteDestination, RouteOwner, Sourced,
    };

    use super::*;

    #[test]
    fn exact_api_versions_are_compatible() {
        assert_eq!(
            ApiVersion::CURRENT.compatibility_with_server(ApiVersion::CURRENT),
            ApiCompatibility::Exact
        );
    }

    #[test]
    fn older_minor_client_is_compatible_with_newer_server() {
        let client = ApiVersion::new(1, 2);
        let server = ApiVersion::new(1, 5);
        assert_eq!(
            client.compatibility_with_server(server),
            ApiCompatibility::Compatible
        );
        assert!(client.is_compatible_with_server(server));
    }

    #[test]
    fn newer_minor_client_is_rejected_by_older_server() {
        let client = ApiVersion::new(1, 5);
        let server = ApiVersion::new(1, 2);
        assert_eq!(
            client.compatibility_with_server(server),
            ApiCompatibility::Incompatible
        );
    }

    #[test]
    fn different_major_versions_are_incompatible() {
        assert!(!ApiVersion::new(1, 9).is_compatible_with_server(ApiVersion::new(2, 0)));
    }

    #[test]
    fn display_is_stable() {
        assert_eq!(ApiVersion::new(12, 34).to_string(), "12.34");
    }

    #[test]
    fn ordering_is_lexicographic_by_major_then_minor() {
        assert_eq!(
            ApiVersion::new(1, 9).cmp(&ApiVersion::new(2, 0)),
            Ordering::Less
        );
    }

    #[test]
    fn dbus_names_are_versioned() {
        assert!(DBUS_SERVICE_NAME.ends_with("Service1"));
        assert!(DBUS_OBJECT_PATH.ends_with("Service1"));
        assert!(DBUS_INTERFACE_NAME.ends_with("Service1"));
    }

    #[test]
    fn commands_keep_friendly_names_separate_from_ids() {
        let name = DisplayName::new("Lab network").unwrap();
        let command = Command::CreateNetwork { name: name.clone() };
        assert_eq!(command, Command::CreateNetwork { name });
    }

    #[test]
    fn capability_events_are_structurally_deterministic() {
        let capabilities = NodeCapabilities {
            panu: Some(Sourced::new(true, CapabilitySource::Measured)),
            ..NodeCapabilities::default()
        };
        let first = Event::CapabilitiesChanged {
            node: NodeId::from_bytes([4; 16]),
            capabilities: capabilities.clone(),
        };
        let second = Event::CapabilitiesChanged {
            node: NodeId::from_bytes([4; 16]),
            capabilities,
        };
        assert_eq!(first, second);
    }

    #[test]
    fn route_events_are_structurally_deterministic() {
        let route = Route {
            destination: RouteDestination::Internet,
            next_hop: NodeId::from_bytes([2; 16]),
            cost: 10,
            owner: RouteOwner::BlueRouteNetwork(NetworkId::from_bytes([3; 16])),
        };
        assert_eq!(
            Event::RouteChanged(route.clone()),
            Event::RouteChanged(route)
        );
    }

    #[test]
    fn internet_sharing_command_is_reserved_in_protocol() {
        assert_eq!(
            Command::SetInternetSharing { enabled: false },
            Command::SetInternetSharing { enabled: false }
        );
    }
}
