//! Typed, read-only desktop-to-daemon boundary.
//!
//! A future Tauri 2 command layer can expose these narrow snapshots without
//! granting web content raw D-Bus access or arbitrary daemon command dispatch.
mod platform_check;

pub use platform_check::{ActionAvailability, PlatformCheck, PlatformIssue};

use blueroute_client::{BlueRouteClient, ClientErrorKind};
use blueroute_core::{HealthLevel, NetworkId, NodeId};
use blueroute_protocol::{Command, DaemonStatus, NetworkSummary, NodeSummary, Response};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopError {
    DaemonUnavailable,
    PermissionDenied,
    Unsupported,
    Operational,
    UnexpectedResponse,
}

impl From<ClientErrorKind> for DesktopError {
    fn from(kind: ClientErrorKind) -> Self {
        match kind {
            ClientErrorKind::Unavailable => Self::DaemonUnavailable,
            ClientErrorKind::Authorization => Self::PermissionDenied,
            ClientErrorKind::Unsupported => Self::Unsupported,
            ClientErrorKind::Operational => Self::Operational,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopOverview {
    pub local_node: Option<NodeId>,
    pub network: Option<NetworkId>,
    pub health: HealthLevel,
    pub adapter_available: Option<bool>,
    pub can_host: Option<bool>,
    pub can_join: Option<bool>,
    /// Remote membership is deliberately unknown until authenticated observations exist.
    pub authenticated_remote_peers: Option<u32>,
    /// v1 has no production Internet sharing.
    pub internet_sharing_available: bool,
}

impl From<DaemonStatus> for DesktopOverview {
    fn from(status: DaemonStatus) -> Self {
        Self {
            local_node: status.local_node,
            network: status.current_network,
            health: status.health,
            adapter_available: status.capabilities.adapter_usable.as_ref().map(|v| v.value),
            can_host: status.capabilities.can_host_pan(),
            can_join: status.capabilities.can_join_pan(),
            authenticated_remote_peers: None,
            internet_sharing_available: false,
        }
    }
}

/// No caller-provided command is accepted. This bridge is read-only by construction.
pub struct DesktopBridge {
    client: BlueRouteClient,
}

impl DesktopBridge {
    /// Connect through the shared client, including its daemon API-version gate.
    pub async fn connect() -> Result<Self, DesktopError> {
        let client = BlueRouteClient::connect()
            .await
            .map_err(|error| DesktopError::from(error.kind()))?;
        Ok(Self { client })
    }

    /// First-run readiness uses only local, version-gated daemon observations.
    /// This never authorizes a network mutation or trusts a discovered peer.
    pub async fn platform_check(&self) -> Result<PlatformCheck, DesktopError> {
        self.client
            .status()
            .await
            .map(|status| PlatformCheck::from_status(&status))
            .map_err(|error| DesktopError::from(error.kind()))
    }

    pub async fn overview(&self) -> Result<DesktopOverview, DesktopError> {
        self.client
            .status()
            .await
            .map(DesktopOverview::from)
            .map_err(|error| DesktopError::from(error.kind()))
    }

    pub async fn networks(&self) -> Result<Vec<NetworkSummary>, DesktopError> {
        let result = self
            .client
            .request(&Command::ListNetworks)
            .await
            .map_err(|error| DesktopError::from(error.kind()))?;
        match result {
            Response::Networks(networks) => Ok(networks),
            _ => Err(DesktopError::UnexpectedResponse),
        }
    }

    pub async fn devices(&self) -> Result<Vec<NodeSummary>, DesktopError> {
        let result = self
            .client
            .request(&Command::ListNodes)
            .await
            .map_err(|error| DesktopError::from(error.kind()))?;
        match result {
            Response::Nodes(nodes) => Ok(nodes),
            _ => Err(DesktopError::UnexpectedResponse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::{CapabilitySource, NodeCapabilities, Sourced};
    use blueroute_protocol::ApiVersion;

    #[test]
    fn overview_never_promotes_local_health_to_authenticated_membership() {
        let capabilities = NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Measured)),
            ..Default::default()
        };
        let status = DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([1; 16])),
            current_network: Some(NetworkId::from_bytes([2; 16])),
            health: HealthLevel::Healthy,
            capabilities,
        };
        let overview = DesktopOverview::from(status);
        assert_eq!(overview.adapter_available, Some(true));
        assert_eq!(overview.authenticated_remote_peers, None);
        assert!(!overview.internet_sharing_available);
        assert_eq!(overview.can_join, None);
    }

    #[test]
    fn errors_are_typed_and_never_leak_raw_daemon_payloads() {
        assert_eq!(
            DesktopError::from(ClientErrorKind::Authorization),
            DesktopError::PermissionDenied
        );
        assert_eq!(
            DesktopError::from(ClientErrorKind::Unavailable),
            DesktopError::DaemonUnavailable
        );
        assert_eq!(
            DesktopError::from(ClientErrorKind::Unsupported),
            DesktopError::Unsupported
        );
        assert_eq!(
            DesktopError::from(ClientErrorKind::Operational),
            DesktopError::Operational
        );
    }
}
