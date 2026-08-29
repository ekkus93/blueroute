/// Describes where a capability value came from.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CapabilitySource {
    Discovered,
    Measured,
    Configured,
    ConservativeDefault,
}

/// A value paired with evidence provenance.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Sourced<T> {
    pub value: T,
    pub source: CapabilitySource,
}

impl<T> Sourced<T> {
    pub const fn new(value: T, source: CapabilitySource) -> Self {
        Self { value, source }
    }
}

/// Linux networking implementation available on a node.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NetworkBackend {
    NetworkManager,
    SystemdNetworkd,
    DirectNetlink,
    Other(String),
}

/// Optional topology-quality information exposed by a platform.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LinkQuality {
    /// Normalized 0-100 score. Interpretation is backend-specific.
    pub score: u8,
}

impl LinkQuality {
    pub const fn new(score: u8) -> Self {
        Self { score }
    }
}

/// Coarse power information that may influence future topology policy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PowerState {
    AcPowered,
    Battery { percent: u8 },
}

/// Capabilities are optional because unknown must remain distinguishable from false.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeCapabilities {
    pub adapter_usable: Option<Sourced<bool>>,
    pub panu: Option<Sourced<bool>>,
    pub nap: Option<Sourced<bool>>,
    pub routing: Option<Sourced<bool>>,
    pub network_backend: Option<Sourced<NetworkBackend>>,
    pub connection_policy_ceiling: Option<Sourced<u16>>,
    pub link_quality: Option<Sourced<LinkQuality>>,
    pub power_state: Option<Sourced<PowerState>>,
    pub has_internet: Option<Sourced<bool>>,
    pub willing_to_share_internet: Option<Sourced<bool>>,
}

impl NodeCapabilities {
    pub fn can_join_pan(&self) -> Option<bool> {
        self.panu.as_ref().map(|value| value.value)
    }

    pub fn can_host_pan(&self) -> Option<bool> {
        self.nap.as_ref().map(|value| value.value)
    }

    pub fn can_route(&self) -> Option<bool> {
        self.routing.as_ref().map(|value| value.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heterogeneous_nodes_need_no_model_specific_type() {
        let client_only = NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Discovered)),
            panu: Some(Sourced::new(true, CapabilitySource::Measured)),
            nap: Some(Sourced::new(false, CapabilitySource::Measured)),
            network_backend: Some(Sourced::new(
                NetworkBackend::NetworkManager,
                CapabilitySource::Discovered,
            )),
            ..NodeCapabilities::default()
        };

        let router = NodeCapabilities {
            panu: Some(Sourced::new(true, CapabilitySource::Measured)),
            nap: Some(Sourced::new(true, CapabilitySource::Measured)),
            routing: Some(Sourced::new(true, CapabilitySource::Configured)),
            connection_policy_ceiling: Some(Sourced::new(4, CapabilitySource::ConservativeDefault)),
            ..NodeCapabilities::default()
        };

        assert_eq!(client_only.can_host_pan(), Some(false));
        assert_eq!(router.can_host_pan(), Some(true));
        assert_eq!(router.can_route(), Some(true));
    }

    #[test]
    fn internet_presence_is_distinct_from_willingness_to_share() {
        let capabilities = NodeCapabilities {
            has_internet: Some(Sourced::new(true, CapabilitySource::Discovered)),
            willing_to_share_internet: Some(Sourced::new(false, CapabilitySource::Configured)),
            ..NodeCapabilities::default()
        };

        assert!(capabilities.has_internet.unwrap().value);
        assert!(!capabilities.willing_to_share_internet.unwrap().value);
    }
}
