#![doc = "Shared BlueRoute protocol and API types."]

use std::fmt;

use blueroute_core::{
    DisplayName, HealthLevel, LinkHealth, LinkId, LinkState, MembershipState, NetworkId,
    NodeCapabilities, NodeId, Reachability, Route,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Version of the local daemon API contract.
///
/// Major versions are incompatible. Minor versions are additive: a server can
/// serve a client with the same major version and a minor version no newer than
/// the server's own minor version.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NetworkSummary {
    pub id: NetworkId,
    pub name: DisplayName,
    pub member_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NodeSummary {
    pub id: NodeId,
    pub name: DisplayName,
    pub reachability: Reachability,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonStatus {
    pub api_version: ApiVersion,
    pub local_node: Option<NodeId>,
    pub current_network: Option<NetworkId>,
    pub health: HealthLevel,
    pub capabilities: NodeCapabilities,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticSnapshot {
    pub api_version: ApiVersion,
    pub health: HealthLevel,
    pub current_network: Option<NetworkId>,
    pub visible_nodes: u32,
    pub capabilities: NodeCapabilities,
}

/// Semantic local-daemon operations shared by all front ends.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
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

/// Stable error boundary for malformed local protocol payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolCodecError {
    message: String,
}

impl ProtocolCodecError {
    fn from_json(error: serde_json::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProtocolCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProtocolCodecError {}

fn encode_json<T: Serialize>(value: &T) -> Result<String, ProtocolCodecError> {
    serde_json::to_string(value).map_err(ProtocolCodecError::from_json)
}

fn decode_json<T: DeserializeOwned>(payload: &str) -> Result<T, ProtocolCodecError> {
    serde_json::from_str(payload).map_err(ProtocolCodecError::from_json)
}

/// Encode one command using the stable compact JSON representation.
pub fn encode_command(command: &Command) -> Result<String, ProtocolCodecError> {
    encode_json(command)
}

/// Decode one command, rejecting malformed payloads and invalid domain values.
pub fn decode_command(payload: &str) -> Result<Command, ProtocolCodecError> {
    decode_json(payload)
}

/// Encode one daemon response using the stable compact JSON representation.
pub fn encode_response(response: &Response) -> Result<String, ProtocolCodecError> {
    encode_json(response)
}

/// Decode one daemon response.
pub fn decode_response(payload: &str) -> Result<Response, ProtocolCodecError> {
    decode_json(payload)
}

/// Encode one daemon event using the stable compact JSON representation.
pub fn encode_event(event: &Event) -> Result<String, ProtocolCodecError> {
    encode_json(event)
}

/// Decode one daemon event.
pub fn decode_event(payload: &str) -> Result<Event, ProtocolCodecError> {
    decode_json(payload)
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

    fn capabilities() -> NodeCapabilities {
        NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Discovered)),
            panu: Some(Sourced::new(true, CapabilitySource::Measured)),
            nap: Some(Sourced::new(false, CapabilitySource::Measured)),
            routing: Some(Sourced::new(true, CapabilitySource::Configured)),
            connection_policy_ceiling: Some(Sourced::new(4, CapabilitySource::ConservativeDefault)),
            ..NodeCapabilities::default()
        }
    }

    fn node_summary(value: u8) -> NodeSummary {
        NodeSummary {
            id: NodeId::from_bytes([value; 16]),
            name: DisplayName::new(format!("Node {value}")).unwrap(),
            reachability: Reachability::DirectLink,
        }
    }

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
    fn command_codec_is_deterministic_and_round_trips_every_variant() {
        let commands = vec![
            Command::GetStatus,
            Command::GetCapabilities,
            Command::ListNetworks,
            Command::CreateNetwork {
                name: DisplayName::new("Lab network").unwrap(),
            },
            Command::JoinNetwork {
                network: NetworkId::from_bytes([1; 16]),
            },
            Command::LeaveNetwork,
            Command::ListNodes,
            Command::GetNode {
                node: NodeId::from_bytes([2; 16]),
            },
            Command::SetDeviceName {
                name: DisplayName::new("Blue laptop").unwrap(),
            },
            Command::StartDiscovery,
            Command::StopDiscovery,
            Command::TrustPeer {
                node: NodeId::from_bytes([3; 16]),
            },
            Command::ForgetPeer {
                node: NodeId::from_bytes([4; 16]),
            },
            Command::GetDiagnostics,
            Command::SetInternetSharing { enabled: false },
        ];

        for command in commands {
            let first = encode_command(&command).unwrap();
            let second = encode_command(&command).unwrap();
            assert_eq!(first, second);
            assert_eq!(decode_command(&first).unwrap(), command);
        }

        assert_eq!(
            encode_command(&Command::CreateNetwork {
                name: DisplayName::new("Lab network").unwrap(),
            })
            .unwrap(),
            r#"{"type":"create_network","data":{"name":"Lab network"}}"#
        );
    }

    #[test]
    fn response_codec_is_deterministic_and_round_trips_representative_payloads() {
        let status = DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([5; 16])),
            current_network: Some(NetworkId::from_bytes([6; 16])),
            health: HealthLevel::Healthy,
            capabilities: capabilities(),
        };
        let diagnostics = DiagnosticSnapshot {
            api_version: ApiVersion::CURRENT,
            health: HealthLevel::Degraded,
            current_network: None,
            visible_nodes: 2,
            capabilities: capabilities(),
        };
        let responses = vec![
            Response::Ack,
            Response::Status(status),
            Response::Capabilities(capabilities()),
            Response::Networks(vec![NetworkSummary {
                id: NetworkId::from_bytes([7; 16]),
                name: DisplayName::new("Lab").unwrap(),
                member_count: 2,
            }]),
            Response::Nodes(vec![node_summary(8)]),
            Response::Node(Some(node_summary(9))),
            Response::Node(None),
            Response::Diagnostics(diagnostics),
        ];

        for response in responses {
            let first = encode_response(&response).unwrap();
            assert_eq!(first, encode_response(&response).unwrap());
            assert_eq!(decode_response(&first).unwrap(), response);
        }
    }

    #[test]
    fn event_codec_is_deterministic_and_round_trips_representative_payloads() {
        let route = Route {
            destination: RouteDestination::Internet,
            next_hop: NodeId::from_bytes([10; 16]),
            cost: 10,
            owner: RouteOwner::BlueRouteNetwork(NetworkId::from_bytes([11; 16])),
        };
        let events = vec![
            Event::NetworkDiscovered(NetworkSummary {
                id: NetworkId::from_bytes([12; 16]),
                name: DisplayName::new("Nearby").unwrap(),
                member_count: 3,
            }),
            Event::NetworkLost(NetworkId::from_bytes([13; 16])),
            Event::NodeChanged(node_summary(14)),
            Event::NodeDisconnected(NodeId::from_bytes([15; 16])),
            Event::CapabilitiesChanged {
                node: NodeId::from_bytes([16; 16]),
                capabilities: capabilities(),
            },
            Event::MembershipChanged {
                network: NetworkId::from_bytes([17; 16]),
                state: MembershipState::Member,
            },
            Event::LinkChanged {
                link: LinkId::from_bytes([18; 16]),
                state: LinkState::Active,
                health: LinkHealth::Healthy,
            },
            Event::TopologyChanged {
                network: NetworkId::from_bytes([19; 16]),
            },
            Event::RouteChanged(route),
            Event::HealthChanged(HealthLevel::Reconnecting),
            Event::AuthorizationFailed {
                operation: "create_network".into(),
            },
            Event::InternetAvailabilityChanged { available: false },
            Event::GatewayAvailabilityChanged {
                node: NodeId::from_bytes([20; 16]),
                available: false,
            },
        ];

        for event in events {
            let first = encode_event(&event).unwrap();
            assert_eq!(first, encode_event(&event).unwrap());
            assert_eq!(decode_event(&first).unwrap(), event);
        }
    }

    #[test]
    fn malformed_payloads_and_invalid_domain_values_fail_closed() {
        assert!(decode_command("not-json").is_err());
        assert!(decode_command(r#"{"type":"unknown"}"#).is_err());
        assert!(
            decode_command(r#"{"type":"create_network","data":{"name":"   "}}"#).is_err()
        );
        assert!(decode_event(r#"{"type":"network_lost","data":"not-an-id"}"#).is_err());
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
