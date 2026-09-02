use std::net::Ipv4Addr;

use crate::{CoreError, DisplayName, ErrorKind};

/// Persistent configuration schema version.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConfigVersion(u32);

impl ConfigVersion {
    pub const CURRENT: Self = Self(1);

    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// IPv4 pool from which routed PAN-segment prefixes can later be allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4AddressPool {
    pub network: Ipv4Addr,
    pub pool_prefix_len: u8,
    pub segment_prefix_len: u8,
}

impl Ipv4AddressPool {
    pub fn new(
        network: Ipv4Addr,
        pool_prefix_len: u8,
        segment_prefix_len: u8,
    ) -> Result<Self, CoreError> {
        let pool = Self {
            network,
            pool_prefix_len,
            segment_prefix_len,
        };
        pool.validate()?;
        Ok(pool)
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if self.pool_prefix_len > 32 {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "IPv4 pool prefix length cannot exceed 32",
            ));
        }
        if self.segment_prefix_len > 30 {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "PAN segment prefix must leave room for host addresses",
            ));
        }
        if self.segment_prefix_len < self.pool_prefix_len {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "PAN segment prefix cannot be broader than the BlueRoute address pool",
            ));
        }

        let address = u32::from(self.network);
        let host_bits = 32_u32 - u32::from(self.pool_prefix_len);
        let host_mask = if host_bits == 32 {
            u32::MAX
        } else if host_bits == 0 {
            0
        } else {
            (1_u32 << host_bits) - 1
        };
        if address & host_mask != 0 {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "IPv4 pool address must be aligned to its prefix",
            ));
        }
        if !pool_is_rfc1918(self.network, self.pool_prefix_len) {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "BlueRoute IPv4 address pool must be fully contained in RFC1918 private space",
            ));
        }
        Ok(())
    }
}


fn pool_is_rfc1918(network: Ipv4Addr, prefix_len: u8) -> bool {
    const PRIVATE_RANGES: [(u32, u8); 3] = [
        (0x0a00_0000, 8),
        (0xac10_0000, 12),
        (0xc0a8_0000, 16),
    ];
    let network = u32::from(network);
    PRIVATE_RANGES.into_iter().any(|(private_network, private_prefix)| {
        if prefix_len < private_prefix {
            return false;
        }
        let mask = u32::MAX << (32 - u32::from(private_prefix));
        network & mask == private_network
    })
}

impl Default for Ipv4AddressPool {
    fn default() -> Self {
        Self {
            network: Ipv4Addr::new(10, 201, 0, 0),
            pool_prefix_len: 16,
            segment_prefix_len: 24,
        }
    }
}

/// Preferred Linux network configuration implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendPreference {
    Auto,
    NetworkManager,
    SystemdNetworkd,
    DirectNetlink,
    Other(String),
}

/// User/policy overrides for future automatic topology planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TopologyPolicy {
    pub automatic_role_selection: bool,
    pub max_active_links: Option<u16>,
    pub prefer_ac_powered_hubs: bool,
}

impl Default for TopologyPolicy {
    fn default() -> Self {
        Self {
            automatic_role_selection: true,
            max_active_links: None,
            prefer_ac_powered_hubs: true,
        }
    }
}

/// Reserved configuration for the post-core Internet gateway feature.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GatewayPolicy {
    pub sharing_enabled: bool,
}

/// Durable daemon configuration. Hardware capability observations do not belong here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonConfig {
    pub version: ConfigVersion,
    pub display_name: DisplayName,
    pub ipv4_address_pool: Ipv4AddressPool,
    pub topology: TopologyPolicy,
    pub backend: BackendPreference,
    pub gateway: GatewayPolicy,
}

impl DaemonConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.version != ConfigVersion::CURRENT {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                format!(
                    "unsupported configuration version {}; expected {}",
                    self.version.get(),
                    ConfigVersion::CURRENT.get()
                ),
            ));
        }

        self.ipv4_address_pool.validate()?;

        if self.topology.max_active_links == Some(0) {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "maximum active links must be greater than zero when configured",
            ));
        }

        if let BackendPreference::Other(name) = &self.backend
            && name.trim().is_empty()
        {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "custom network backend name cannot be empty",
            ));
        }

        if self.gateway.sharing_enabled {
            return Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "Internet sharing is reserved but not implemented in configuration version 1",
            ));
        }

        Ok(())
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            version: ConfigVersion::CURRENT,
            display_name: DisplayName::new("BlueRoute Device")
                .expect("the built-in BlueRoute display name is valid"),
            ipv4_address_pool: Ipv4AddressPool::default(),
            topology: TopologyPolicy::default(),
            backend: BackendPreference::Auto,
            gateway: GatewayPolicy::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_configuration_is_valid_and_gateway_is_off() {
        let config = DaemonConfig::default();
        assert!(config.validate().is_ok());
        assert!(!config.gateway.sharing_enabled);
        assert_eq!(config.version, ConfigVersion::CURRENT);
    }

    #[test]
    fn address_pool_requires_aligned_network() {
        assert!(Ipv4AddressPool::new(Ipv4Addr::new(10, 201, 1, 0), 16, 24).is_err());
    }

    #[test]
    fn address_pool_requires_segment_prefix_inside_pool() {
        assert!(Ipv4AddressPool::new(Ipv4Addr::new(10, 201, 0, 0), 24, 16).is_err());
    }

    #[test]
    fn address_pool_requires_rfc1918_private_space() {
        assert!(Ipv4AddressPool::new(Ipv4Addr::new(192, 0, 2, 0), 24, 24).is_err());
        assert!(Ipv4AddressPool::new(Ipv4Addr::new(100, 64, 0, 0), 10, 24).is_err());
        assert!(Ipv4AddressPool::new(Ipv4Addr::new(172, 16, 0, 0), 12, 24).is_ok());
        assert!(Ipv4AddressPool::new(Ipv4Addr::new(192, 168, 0, 0), 16, 24).is_ok());
    }

    #[test]
    fn zero_link_override_is_rejected() {
        let mut config = DaemonConfig::default();
        config.topology.max_active_links = Some(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn unknown_configuration_version_is_rejected() {
        let config = DaemonConfig {
            version: ConfigVersion::new(99),
            ..DaemonConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn internet_sharing_cannot_be_enabled_before_gateway_phase() {
        let mut config = DaemonConfig::default();
        config.gateway.sharing_enabled = true;
        let error = config.validate().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::CapabilityUnavailable);
    }

    #[test]
    fn configuration_has_no_computer_model_policy() {
        let config = DaemonConfig::default();
        assert_eq!(config.backend, BackendPreference::Auto);
        assert_eq!(config.topology.max_active_links, None);
    }
}
