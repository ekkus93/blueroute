use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::{CoreError, ErrorKind, NetworkId, NodeId, SegmentId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct IpPrefix {
    pub address: IpAddr,
    pub prefix_len: u8,
}

impl IpPrefix {
    pub fn new(address: IpAddr, prefix_len: u8) -> Result<Self, CoreError> {
        let maximum = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix_len > maximum {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                format!("prefix length {prefix_len} is invalid for {address}"),
            ));
        }
        Ok(Self {
            address,
            prefix_len,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteDestination {
    Node(NodeId),
    Segment(SegmentId),
    Prefix(IpPrefix),
    /// Reserved for later gateway/default-route support.
    Internet,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteOwner {
    BlueRouteNetwork(NetworkId),
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Route {
    pub destination: RouteDestination,
    pub next_hop: NodeId,
    pub cost: u32,
    pub owner: RouteOwner,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn prefix_validation_supports_ipv4_and_ipv6() {
        assert!(IpPrefix::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 32).is_ok());
        assert!(IpPrefix::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 33).is_err());
        assert!(IpPrefix::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 128).is_ok());
        assert!(IpPrefix::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 129).is_err());
    }

    #[test]
    fn route_model_reserves_internet_destination() {
        let route = Route {
            destination: RouteDestination::Internet,
            next_hop: NodeId::from_bytes([2; 16]),
            cost: 10,
            owner: RouteOwner::BlueRouteNetwork(NetworkId::from_bytes([3; 16])),
        };
        assert_eq!(route.destination, RouteDestination::Internet);
    }
}
