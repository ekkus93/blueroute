//! P8-002 deterministic reservation failure and recovery tests.
//! No privileged networking or Bluetooth behavior is exercised.

use std::net::{IpAddr, Ipv4Addr};

use blueroute_core::{ErrorKind, IpPrefix, Ipv4AddressPool, SegmentId, SegmentIpv4Allocator};

fn segment(value: u8) -> SegmentId {
    SegmentId::from_bytes([value; 16])
}

fn prefix(last: u8) -> IpPrefix {
    IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 201, 8, last)), 30).unwrap()
}

#[test]
fn exhausted_pool_preserves_reservations_and_recovers_after_conflict_clears() {
    let pool = Ipv4AddressPool::new(Ipv4Addr::new(10, 201, 8, 0), 29, 30).unwrap();
    let mut allocator = SegmentIpv4Allocator::new(pool).unwrap();
    assert_eq!(
        allocator.reserve_exact(segment(1), prefix(0), []).unwrap(),
        prefix(0)
    );

    assert_eq!(
        allocator
            .allocate(segment(2), [prefix(4)])
            .unwrap_err()
            .kind(),
        ErrorKind::AddressConflict
    );
    assert_eq!(allocator.len(), 1);
    assert_eq!(allocator.get(segment(1)), Some(prefix(0)));
    assert_eq!(allocator.get(segment(2)), None);

    assert_eq!(allocator.allocate(segment(2), []).unwrap(), prefix(4));
    assert_eq!(allocator.len(), 2);
    assert_eq!(allocator.allocate(segment(1), []).unwrap(), prefix(0));
    assert_eq!(
        allocator.allocate(segment(3), []).unwrap_err().kind(),
        ErrorKind::AddressConflict
    );
    assert_eq!(allocator.release(segment(2)), Some(prefix(4)));
    assert_eq!(allocator.allocate(segment(3), []).unwrap(), prefix(4));
    assert_eq!(allocator.get(segment(1)), Some(prefix(0)));
}
