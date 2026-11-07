//! First-run capability explanation for the future desktop UI.
//! The platform check never guesses that a missing observation is a working radio.
use blueroute_core::{HealthLevel, NetworkBackend, NodeCapabilities};
use blueroute_protocol::DaemonStatus;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionAvailability {
    /// An observed local prerequisite exists; final policy checks still apply.
    PrerequisitesObserved,
    /// An explicit missing prerequisite prevents even offering this workflow.
    Blocked,
    /// The daemon has not reported enough evidence to decide.
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformIssue {
    AdapterMissingOrDisabled,
    AdapterUnverified,
    PanClientUnsupported,
    PanHostUnsupported,
    PanClientUnverified,
    PanHostUnverified,
    NetworkBackendUnsupported,
    NetworkBackendUnverified,
    DaemonDegraded,
    DaemonHealthUnknown,
    /// Authenticated outbound PANU joining is not production-ready.
    ProductionJoinUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlatformCheck {
    pub summary: &'static str,
    pub issues: Vec<PlatformIssue>,
    pub create: ActionAvailability,
    pub join: ActionAvailability,
    /// Must remain false until the full authenticated production join lifecycle is available.
    pub join_enabled: bool,
    pub internet_sharing_enabled: bool,
}

fn observed_capability(
    value: Option<bool>,
    unsupported: PlatformIssue,
    unknown: PlatformIssue,
    issues: &mut Vec<PlatformIssue>,
) -> ActionAvailability {
    match value {
        Some(true) => ActionAvailability::PrerequisitesObserved,
        Some(false) => {
            issues.push(unsupported);
            ActionAvailability::Blocked
        }
        None => {
            issues.push(unknown);
            ActionAvailability::Unknown
        }
    }
}

fn combine(a: ActionAvailability, b: ActionAvailability) -> ActionAvailability {
    match (a, b) {
        (ActionAvailability::Blocked, _) | (_, ActionAvailability::Blocked) => {
            ActionAvailability::Blocked
        }
        (ActionAvailability::Unknown, _) | (_, ActionAvailability::Unknown) => {
            ActionAvailability::Unknown
        }
        _ => ActionAvailability::PrerequisitesObserved,
    }
}

fn backend(caps: &NodeCapabilities, issues: &mut Vec<PlatformIssue>) -> ActionAvailability {
    match caps.network_backend.as_ref().map(|v| &v.value) {
        Some(NetworkBackend::NetworkManager) => ActionAvailability::PrerequisitesObserved,
        Some(_) => {
            issues.push(PlatformIssue::NetworkBackendUnsupported);
            ActionAvailability::Blocked
        }
        None => {
            issues.push(PlatformIssue::NetworkBackendUnverified);
            ActionAvailability::Unknown
        }
    }
}

impl PlatformCheck {
    pub fn from_status(status: &DaemonStatus) -> Self {
        let mut issues = Vec::new();
        let adapter = observed_capability(
            status.capabilities.adapter_usable.as_ref().map(|v| v.value),
            PlatformIssue::AdapterMissingOrDisabled,
            PlatformIssue::AdapterUnverified,
            &mut issues,
        );
        let host = observed_capability(
            status.capabilities.can_host_pan(),
            PlatformIssue::PanHostUnsupported,
            PlatformIssue::PanHostUnverified,
            &mut issues,
        );
        let client = observed_capability(
            status.capabilities.can_join_pan(),
            PlatformIssue::PanClientUnsupported,
            PlatformIssue::PanClientUnverified,
            &mut issues,
        );
        let network = backend(&status.capabilities, &mut issues);
        let health = match status.health {
            HealthLevel::Healthy => ActionAvailability::PrerequisitesObserved,
            HealthLevel::Degraded | HealthLevel::Reconnecting | HealthLevel::Error => {
                issues.push(PlatformIssue::DaemonDegraded);
                ActionAvailability::Blocked
            }
        };
        // Host/create and outbound join require different PAN roles.
        let create = combine(combine(adapter, host), combine(network, health));
        let join = combine(combine(adapter, client), combine(network, health));
        issues.push(PlatformIssue::ProductionJoinUnavailable);
        let summary = if create == ActionAvailability::PrerequisitesObserved {
            "Local host prerequisites observed; creation still requires daemon policy authorization."
        } else {
            "Check Bluetooth, PAN role, daemon health, and the network backend before creating a network."
        };
        Self {
            summary,
            issues,
            create,
            join,
            join_enabled: false,
            internet_sharing_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::{CapabilitySource, NetworkId, NodeId, Sourced};
    use blueroute_protocol::ApiVersion;

    fn status() -> DaemonStatus {
        DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([1; 16])),
            current_network: Some(NetworkId::from_bytes([2; 16])),
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        }
    }

    fn observed<T>(value: T) -> Option<Sourced<T>> {
        Some(Sourced::new(value, CapabilitySource::Measured))
    }

    #[test]
    fn unknown_is_not_treated_as_supported() {
        let check = PlatformCheck::from_status(&status());
        assert_eq!(check.create, ActionAvailability::Unknown);
        assert_eq!(check.join, ActionAvailability::Unknown);
        assert!(check.issues.contains(&PlatformIssue::AdapterUnverified));
        assert!(
            check
                .issues
                .contains(&PlatformIssue::NetworkBackendUnverified)
        );
        assert!(!check.join_enabled);
        assert!(!check.internet_sharing_enabled);
    }

    #[test]
    fn capable_host_is_explained_but_join_remains_disabled() {
        let mut daemon = status();
        daemon.capabilities.adapter_usable = observed(true);
        daemon.capabilities.nap = observed(true);
        daemon.capabilities.panu = observed(true);
        daemon.capabilities.network_backend = observed(NetworkBackend::NetworkManager);
        let check = PlatformCheck::from_status(&daemon);
        assert_eq!(check.create, ActionAvailability::PrerequisitesObserved);
        assert_eq!(check.join, ActionAvailability::PrerequisitesObserved);
        assert!(!check.join_enabled);
        assert!(
            check
                .issues
                .contains(&PlatformIssue::ProductionJoinUnavailable)
        );
        assert!(check.summary.contains("policy authorization"));
    }

    #[test]
    fn missing_adapter_and_unsupported_backend_block_actions() {
        let mut daemon = status();
        daemon.capabilities.adapter_usable = observed(false);
        daemon.capabilities.nap = observed(true);
        daemon.capabilities.panu = observed(true);
        daemon.capabilities.network_backend = observed(NetworkBackend::SystemdNetworkd);
        let check = PlatformCheck::from_status(&daemon);
        assert_eq!(check.create, ActionAvailability::Blocked);
        assert_eq!(check.join, ActionAvailability::Blocked);
        assert!(
            check
                .issues
                .contains(&PlatformIssue::AdapterMissingOrDisabled)
        );
        assert!(
            check
                .issues
                .contains(&PlatformIssue::NetworkBackendUnsupported)
        );
    }

    #[test]
    fn pan_role_asymmetry_and_daemon_degradation_are_not_hidden() {
        let mut daemon = status();
        daemon.capabilities.adapter_usable = observed(true);
        daemon.capabilities.nap = observed(false);
        daemon.capabilities.panu = observed(true);
        daemon.capabilities.network_backend = observed(NetworkBackend::NetworkManager);
        let check = PlatformCheck::from_status(&daemon);
        assert_eq!(check.create, ActionAvailability::Blocked);
        assert_eq!(check.join, ActionAvailability::PrerequisitesObserved);
        assert!(check.issues.contains(&PlatformIssue::PanHostUnsupported));
        daemon.health = HealthLevel::Degraded;
        let check = PlatformCheck::from_status(&daemon);
        assert_eq!(check.join, ActionAvailability::Blocked);
        assert!(check.issues.contains(&PlatformIssue::DaemonDegraded));
    }
}
