use std::net::{IpAddr, Ipv4Addr};

use crate::{CoreError, ErrorKind, IpPrefix, Ipv4AddressPool, NetworkId};

/// Deterministic IPv4 addresses for the initial single-star topology.
///
/// The subnet is a pure function of `NetworkId` and the configured pool so a joining node can
/// derive the bootstrap address plan from the discovered logical network identity without using
/// Bluetooth metadata as identity or requiring durable allocation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4StarAddressPlan {
    pub segment: IpPrefix,
    pub host: IpPrefix,
    pub first_client: IpPrefix,
}

impl Ipv4StarAddressPlan {
    pub fn for_network(network: NetworkId, pool: Ipv4AddressPool) -> Result<Self, CoreError> {
        pool.validate()?;
        let segment_bits = pool.segment_prefix_len - pool.pool_prefix_len;
        let segment_count = 1_u32 << u32::from(segment_bits);
        let bytes = network.as_bytes();
        let selector = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % segment_count;
        let segment_host_bits = 32_u32 - u32::from(pool.segment_prefix_len);
        let segment_offset = selector << segment_host_bits;
        let segment_network = u32::from(pool.network)
            .checked_add(segment_offset)
            .ok_or_else(|| CoreError::new(ErrorKind::InvalidInput, "IPv4 pool overflow"))?;
        let host = segment_network
            .checked_add(1)
            .ok_or_else(|| CoreError::new(ErrorKind::InvalidInput, "IPv4 host address overflow"))?;
        let first_client = segment_network.checked_add(2).ok_or_else(|| {
            CoreError::new(ErrorKind::InvalidInput, "IPv4 client address overflow")
        })?;

        Ok(Self {
            segment: IpPrefix::new(
                IpAddr::V4(Ipv4Addr::from(segment_network)),
                pool.segment_prefix_len,
            )?,
            host: IpPrefix::new(IpAddr::V4(Ipv4Addr::from(host)), pool.segment_prefix_len)?,
            first_client: IpPrefix::new(
                IpAddr::V4(Ipv4Addr::from(first_client)),
                pool.segment_prefix_len,
            )?,
        })
    }
}

/// Rejects an IPv4 segment that overlaps any active non-default IPv4 prefix.
///
/// A default route (`0.0.0.0/0`) is intentionally not a collision: Linux can install a more
/// specific connected BlueRoute route alongside an ordinary default route. IPv6 prefixes are
/// irrelevant to this IPv4-only phase and are ignored.
pub fn ensure_ipv4_segment_available(
    candidate: IpPrefix,
    active: impl IntoIterator<Item = IpPrefix>,
) -> Result<(), CoreError> {
    let candidate = normalized_ipv4_prefix(candidate)?;
    for prefix in active {
        if !prefix.address.is_ipv4() || prefix.prefix_len == 0 {
            continue;
        }
        let prefix = normalized_ipv4_prefix(prefix)?;
        if ipv4_prefixes_overlap(candidate, prefix) {
            return Err(CoreError::with_diagnostic(
                ErrorKind::AddressConflict,
                "BlueRoute IPv4 segment overlaps an active local network",
                format!(
                    "candidate={}/{} conflicting={}/{}",
                    candidate.address, candidate.prefix_len, prefix.address, prefix.prefix_len
                ),
            ));
        }
    }
    Ok(())
}

pub fn normalized_ipv4_prefix(prefix: IpPrefix) -> Result<IpPrefix, CoreError> {
    let IpAddr::V4(address) = prefix.address else {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "expected an IPv4 prefix",
        ));
    };
    let mask = ipv4_mask(prefix.prefix_len)?;
    IpPrefix::new(
        IpAddr::V4(Ipv4Addr::from(u32::from(address) & mask)),
        prefix.prefix_len,
    )
}

fn ipv4_prefixes_overlap(left: IpPrefix, right: IpPrefix) -> bool {
    let IpAddr::V4(left_address) = left.address else {
        return false;
    };
    let IpAddr::V4(right_address) = right.address else {
        return false;
    };
    let left_host_bits = 32_u32 - u32::from(left.prefix_len);
    let right_host_bits = 32_u32 - u32::from(right.prefix_len);
    let left_start = u32::from(left_address);
    let right_start = u32::from(right_address);
    let left_end = left_start | host_mask(left_host_bits);
    let right_end = right_start | host_mask(right_host_bits);
    left_start <= right_end && right_start <= left_end
}

fn ipv4_mask(prefix_len: u8) -> Result<u32, CoreError> {
    if prefix_len > 32 {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "IPv4 prefix length cannot exceed 32",
        ));
    }
    Ok(if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    })
}

fn host_mask(host_bits: u32) -> u32 {
    if host_bits == 32 {
        u32::MAX
    } else if host_bits == 0 {
        0
    } else {
        (1_u32 << host_bits) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(value: [u8; 4], prefix_len: u8) -> IpPrefix {
        IpPrefix::new(IpAddr::V4(Ipv4Addr::from(value)), prefix_len).unwrap()
    }

    #[test]
    fn network_identity_deterministically_selects_host_and_first_client() {
        let network =
            NetworkId::from_bytes([0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let plan = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default()).unwrap();
        assert_eq!(plan.segment.prefix_len, 24);
        assert_eq!(plan.host.prefix_len, 24);
        assert_eq!(plan.first_client.prefix_len, 24);
        let IpAddr::V4(segment) = plan.segment.address else {
            panic!("segment must be IPv4");
        };
        let IpAddr::V4(host) = plan.host.address else {
            panic!("host must be IPv4");
        };
        let IpAddr::V4(client) = plan.first_client.address else {
            panic!("client must be IPv4");
        };
        assert_eq!(segment.octets()[0..2], [10, 201]);
        assert_eq!(host.octets()[0..2], [10, 201]);
        assert_eq!(client.octets()[0..2], [10, 201]);
        assert_eq!(host.octets()[3], 1);
        assert_eq!(client.octets()[3], 2);
    }

    #[test]
    fn conflict_detection_normalizes_host_prefixes_and_ignores_default_route() {
        let candidate = v4([10, 201, 44, 1], 24);
        ensure_ipv4_segment_available(candidate, [v4([0, 0, 0, 0], 0)]).unwrap();

        let error =
            ensure_ipv4_segment_available(candidate, [v4([10, 201, 44, 99], 24)]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AddressConflict);
        assert!(error.diagnostic().unwrap().contains("10.201.44.0/24"));
    }

    #[test]
    fn broader_and_narrower_overlaps_are_rejected() {
        let candidate = v4([10, 201, 44, 0], 24);
        assert_eq!(
            ensure_ipv4_segment_available(candidate, [v4([10, 201, 0, 0], 16)])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
        assert_eq!(
            ensure_ipv4_segment_available(candidate, [v4([10, 201, 44, 128], 25)])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
        ensure_ipv4_segment_available(candidate, [v4([10, 201, 45, 0], 24)]).unwrap();
    }

    #[test]
    fn allocation_is_stateless_across_repeated_calls() {
        let network = NetworkId::from_bytes([0x9a; 16]);
        let first = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default()).unwrap();
        let second = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default()).unwrap();
        assert_eq!(first, second);
    }
}
