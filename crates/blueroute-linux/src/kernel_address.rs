use std::net::{IpAddr, Ipv4Addr};

use blueroute_core::{CoreError, ErrorKind, IpPrefix};
use rustix::fd::OwnedFd;
use rustix::net::netdevice;
use rustix::net::netlink::SocketAddrNetlink;
use rustix::net::{
    AddressFamily, RecvFlags, SendFlags, SocketType, bind, connect, recv, send, socket,
};

use crate::{InterfaceAddress, NetworkInterfaceHandle};

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const NLM_F_REQUEST: u16 = 0x0001;
const NLM_F_ACK: u16 = 0x0004;
const NLM_F_ROOT: u16 = 0x0100;
const NLM_F_MATCH: u16 = 0x0200;
const NLM_F_EXCL: u16 = 0x0200;
const NLM_F_CREATE: u16 = 0x0400;
const NLM_F_DUMP: u16 = NLM_F_ROOT | NLM_F_MATCH;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const NLA_TYPE_MASK: u16 = 0x3fff;
const NETLINK_HEADER_LEN: usize = 16;
const IFADDR_MESSAGE_LEN: usize = 8;
const ATTRIBUTE_HEADER_LEN: usize = 4;
const REQUEST_SEQUENCE: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KernelAddressLease {
    address: InterfaceAddress,
    interface_index: u32,
}

impl KernelAddressLease {
    pub fn address(&self) -> &InterfaceAddress {
        &self.address
    }

    pub const fn interface_index(&self) -> u32 {
        self.interface_index
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KernelAddressBackend;

impl KernelAddressBackend {
    pub fn ensure_panu_address(
        &self,
        address: InterfaceAddress,
    ) -> Result<KernelAddressLease, CoreError> {
        let requested = ipv4_prefix(address.prefix)?;
        let socket = route_socket()?;
        let interface_index = interface_index(&socket, &address.interface)?;
        let existing = ipv4_addresses(&socket, interface_index)?;
        if !existing.is_empty() {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "refusing to configure a PANU interface that already has IPv4 state",
                format!(
                    "interface={} ifindex={} existing={}",
                    address.interface.as_str(),
                    interface_index,
                    format_prefixes(&existing)
                ),
            ));
        }

        change_address(
            &socket,
            RTM_NEWADDR,
            NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL,
            interface_index,
            requested,
        )?;

        let applied = ipv4_addresses(&socket, interface_index)?;
        if applied.len() == 1 && applied[0] == requested {
            return Ok(KernelAddressLease {
                address,
                interface_index,
            });
        }

        let cleanup = change_address(
            &socket,
            RTM_DELADDR,
            NLM_F_REQUEST | NLM_F_ACK,
            interface_index,
            requested,
        );
        Err(CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "kernel PANU address application did not converge to the requested state",
            format!(
                "interface={} ifindex={} requested={}/{} observed={} cleanup={}",
                address.interface.as_str(),
                interface_index,
                requested.address,
                requested.prefix_len,
                format_prefixes(&applied),
                cleanup
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "ok".to_owned())
            ),
        ))
    }

    pub fn remove_panu_address(&self, lease: KernelAddressLease) -> Result<(), CoreError> {
        let requested = ipv4_prefix(lease.address.prefix)?;
        let socket = route_socket()?;
        let current_index = match netdevice::name_to_index(&socket, lease.address.interface.as_str()) {
            Ok(index) => index,
            Err(error) if error == rustix::io::Errno::NODEV => return Ok(()),
            Err(error) => {
                return Err(CoreError::with_diagnostic(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to resolve the PANU interface during address cleanup",
                    format!(
                        "interface={} error={error}",
                        lease.address.interface.as_str()
                    ),
                ));
            }
        };

        if current_index != lease.interface_index {
            return Ok(());
        }

        let existing = ipv4_addresses(&socket, current_index)?;
        if !existing.contains(&requested) {
            return Ok(());
        }

        change_address(
            &socket,
            RTM_DELADDR,
            NLM_F_REQUEST | NLM_F_ACK,
            current_index,
            requested,
        )?;
        let remaining = ipv4_addresses(&socket, current_index)?;
        if remaining.contains(&requested) {
            return Err(CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                "kernel PANU address removal did not converge",
                format!(
                    "interface={} ifindex={} address={}/{} remaining={}",
                    lease.address.interface.as_str(),
                    current_index,
                    requested.address,
                    requested.prefix_len,
                    format_prefixes(&remaining)
                ),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ipv4InterfacePrefix {
    address: Ipv4Addr,
    prefix_len: u8,
}

fn ipv4_prefix(prefix: IpPrefix) -> Result<Ipv4InterfacePrefix, CoreError> {
    match prefix.address {
        IpAddr::V4(address) if prefix.prefix_len <= 32 => Ok(Ipv4InterfacePrefix {
            address,
            prefix_len: prefix.prefix_len,
        }),
        _ => Err(CoreError::new(
            ErrorKind::InvalidInput,
            "kernel PANU address backend currently accepts only IPv4 prefixes",
        )),
    }
}

fn route_socket() -> Result<OwnedFd, CoreError> {
    let socket = socket(AddressFamily::NETLINK, SocketType::RAW, None).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to open a NETLINK_ROUTE socket",
            error.to_string(),
        )
    })?;
    let local = SocketAddrNetlink::new(0, 0);
    bind(&socket, &local).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to bind a NETLINK_ROUTE socket",
            error.to_string(),
        )
    })?;
    let kernel = SocketAddrNetlink::new(0, 0);
    connect(&socket, &kernel).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to connect a NETLINK_ROUTE socket to the kernel",
            error.to_string(),
        )
    })?;
    Ok(socket)
}

fn interface_index(
    socket: &OwnedFd,
    interface: &NetworkInterfaceHandle,
) -> Result<u32, CoreError> {
    netdevice::name_to_index(socket, interface.as_str()).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to resolve the PANU kernel interface",
            format!("interface={} error={error}", interface.as_str()),
        )
    })
}

fn ipv4_addresses(
    socket: &OwnedFd,
    interface_index: u32,
) -> Result<Vec<Ipv4InterfacePrefix>, CoreError> {
    let request = address_dump_request();
    send_exact(socket, &request)?;

    let mut addresses = Vec::new();
    let mut buffer = vec![0_u8; 65_536];
    loop {
        let (_, received) = recv(socket, &mut buffer[..], RecvFlags::empty()).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                "failed to read a NETLINK_ROUTE address response",
                error.to_string(),
            )
        })?;
        let mut offset = 0;
        while offset < received {
            let message = netlink_message(&buffer[offset..received])?;
            if message.sequence != REQUEST_SEQUENCE {
                return Err(protocol_error(format!(
                    "unexpected netlink sequence {}; expected {REQUEST_SEQUENCE}",
                    message.sequence
                )));
            }
            match message.message_type {
                NLMSG_DONE => {
                    addresses.sort_by_key(|prefix| (prefix.address, prefix.prefix_len));
                    addresses.dedup();
                    return Ok(addresses);
                }
                NLMSG_ERROR => parse_netlink_ack(message.payload)?,
                RTM_NEWADDR => {
                    if let Some(prefix) = parse_ipv4_address(message.payload, interface_index)? {
                        addresses.push(prefix);
                    }
                }
                _ => {}
            }
            offset += align4(message.length);
        }
    }
}

fn change_address(
    socket: &OwnedFd,
    message_type: u16,
    flags: u16,
    interface_index: u32,
    prefix: Ipv4InterfacePrefix,
) -> Result<(), CoreError> {
    let request = address_change_request(message_type, flags, interface_index, prefix);
    send_exact(socket, &request)?;
    let mut buffer = vec![0_u8; 16_384];
    loop {
        let (_, received) = recv(socket, &mut buffer[..], RecvFlags::empty()).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                "failed to read a NETLINK_ROUTE address acknowledgement",
                error.to_string(),
            )
        })?;
        let mut offset = 0;
        while offset < received {
            let message = netlink_message(&buffer[offset..received])?;
            if message.sequence != REQUEST_SEQUENCE {
                return Err(protocol_error(format!(
                    "unexpected netlink sequence {}; expected {REQUEST_SEQUENCE}",
                    message.sequence
                )));
            }
            if message.message_type == NLMSG_ERROR {
                return parse_netlink_ack(message.payload);
            }
            offset += align4(message.length);
        }
    }
}

fn send_exact(socket: &OwnedFd, request: &[u8]) -> Result<(), CoreError> {
    let sent = send(socket, request, SendFlags::empty()).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to send a NETLINK_ROUTE request",
            error.to_string(),
        )
    })?;
    if sent != request.len() {
        return Err(CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "NETLINK_ROUTE request was only partially sent",
            format!("sent={sent} expected={}", request.len()),
        ));
    }
    Ok(())
}

struct NetlinkMessage<'a> {
    length: usize,
    message_type: u16,
    sequence: u32,
    payload: &'a [u8],
}

fn netlink_message(bytes: &[u8]) -> Result<NetlinkMessage<'_>, CoreError> {
    if bytes.len() < NETLINK_HEADER_LEN {
        return Err(protocol_error("truncated netlink header"));
    }
    let length = read_u32(&bytes[0..4])? as usize;
    if length < NETLINK_HEADER_LEN || length > bytes.len() {
        return Err(protocol_error(format!(
            "invalid netlink message length {length} for {} available bytes",
            bytes.len()
        )));
    }
    Ok(NetlinkMessage {
        length,
        message_type: read_u16(&bytes[4..6])?,
        sequence: read_u32(&bytes[8..12])?,
        payload: &bytes[NETLINK_HEADER_LEN..length],
    })
}

fn parse_netlink_ack(payload: &[u8]) -> Result<(), CoreError> {
    if payload.len() < 4 {
        return Err(protocol_error("truncated netlink acknowledgement"));
    }
    let raw_error = i32::from_ne_bytes(
        payload[0..4]
            .try_into()
            .map_err(|_| protocol_error("invalid netlink acknowledgement"))?,
    );
    if raw_error == 0 {
        return Ok(());
    }
    let errno = raw_error.checked_neg().unwrap_or(i32::MAX);
    let kind = if errno == 17 {
        ErrorKind::InvalidState
    } else {
        ErrorKind::NetworkBackendUnavailable
    };
    Err(CoreError::with_diagnostic(
        kind,
        "kernel rejected a BlueRoute PANU address operation",
        format!("errno={errno}"),
    ))
}

fn parse_ipv4_address(
    payload: &[u8],
    expected_index: u32,
) -> Result<Option<Ipv4InterfacePrefix>, CoreError> {
    if payload.len() < IFADDR_MESSAGE_LEN {
        return Err(protocol_error("truncated rtnetlink address message"));
    }
    if payload[0] != AddressFamily::INET.as_raw() as u8 {
        return Ok(None);
    }
    let prefix_len = payload[1];
    let interface_index = read_u32(&payload[4..8])?;
    if interface_index != expected_index {
        return Ok(None);
    }

    let mut address = None;
    let mut local = None;
    let mut offset = IFADDR_MESSAGE_LEN;
    while offset < payload.len() {
        if payload.len() - offset < ATTRIBUTE_HEADER_LEN {
            return Err(protocol_error("truncated rtnetlink address attribute"));
        }
        let length = read_u16(&payload[offset..offset + 2])? as usize;
        let attribute_type = read_u16(&payload[offset + 2..offset + 4])? & NLA_TYPE_MASK;
        if length < ATTRIBUTE_HEADER_LEN || offset + length > payload.len() {
            return Err(protocol_error("invalid rtnetlink address attribute length"));
        }
        let value = &payload[offset + ATTRIBUTE_HEADER_LEN..offset + length];
        if matches!(attribute_type, IFA_LOCAL | IFA_ADDRESS) && value.len() == 4 {
            let parsed = Ipv4Addr::new(value[0], value[1], value[2], value[3]);
            if attribute_type == IFA_LOCAL {
                local = Some(parsed);
            } else {
                address = Some(parsed);
            }
        }
        offset += align4(length);
    }

    Ok(local.or(address).map(|address| Ipv4InterfacePrefix {
        address,
        prefix_len,
    }))
}

fn address_dump_request() -> Vec<u8> {
    let mut request = Vec::with_capacity(NETLINK_HEADER_LEN + IFADDR_MESSAGE_LEN);
    push_u32(
        &mut request,
        (NETLINK_HEADER_LEN + IFADDR_MESSAGE_LEN) as u32,
    );
    push_u16(&mut request, RTM_GETADDR);
    push_u16(&mut request, NLM_F_REQUEST | NLM_F_DUMP);
    push_u32(&mut request, REQUEST_SEQUENCE);
    push_u32(&mut request, 0);
    request.push(AddressFamily::INET.as_raw() as u8);
    request.extend_from_slice(&[0, 0, 0]);
    push_u32(&mut request, 0);
    request
}

fn address_change_request(
    message_type: u16,
    flags: u16,
    interface_index: u32,
    prefix: Ipv4InterfacePrefix,
) -> Vec<u8> {
    let mut request = vec![0_u8; NETLINK_HEADER_LEN];
    request.push(AddressFamily::INET.as_raw() as u8);
    request.push(prefix.prefix_len);
    request.extend_from_slice(&[0, 0]);
    push_u32(&mut request, interface_index);
    push_attribute(&mut request, IFA_LOCAL, &prefix.address.octets());
    push_attribute(&mut request, IFA_ADDRESS, &prefix.address.octets());
    let length = request.len() as u32;
    request[0..4].copy_from_slice(&length.to_ne_bytes());
    request[4..6].copy_from_slice(&message_type.to_ne_bytes());
    request[6..8].copy_from_slice(&flags.to_ne_bytes());
    request[8..12].copy_from_slice(&REQUEST_SEQUENCE.to_ne_bytes());
    request[12..16].copy_from_slice(&0_u32.to_ne_bytes());
    request
}

fn push_attribute(buffer: &mut Vec<u8>, attribute_type: u16, value: &[u8]) {
    let length = ATTRIBUTE_HEADER_LEN + value.len();
    push_u16(buffer, length as u16);
    push_u16(buffer, attribute_type);
    buffer.extend_from_slice(value);
    while !buffer.len().is_multiple_of(4) {
        buffer.push(0);
    }
}

fn push_u16(buffer: &mut Vec<u8>, value: u16) {
    buffer.extend_from_slice(&value.to_ne_bytes());
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_ne_bytes());
}

fn read_u16(bytes: &[u8]) -> Result<u16, CoreError> {
    Ok(u16::from_ne_bytes(
        bytes
            .try_into()
            .map_err(|_| protocol_error("invalid two-byte netlink field"))?,
    ))
}

fn read_u32(bytes: &[u8]) -> Result<u32, CoreError> {
    Ok(u32::from_ne_bytes(
        bytes
            .try_into()
            .map_err(|_| protocol_error("invalid four-byte netlink field"))?,
    ))
}

const fn align4(length: usize) -> usize {
    (length + 3) & !3
}

fn protocol_error(diagnostic: impl Into<String>) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::ProtocolError,
        "received malformed rtnetlink data from the kernel",
        diagnostic,
    )
}

fn format_prefixes(prefixes: &[Ipv4InterfacePrefix]) -> String {
    if prefixes.is_empty() {
        return "<none>".to_owned();
    }
    prefixes
        .iter()
        .map(|prefix| format!("{}/{}", prefix.address, prefix.prefix_len))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_change_request_encodes_ipv4_and_interface_index() {
        let prefix = Ipv4InterfacePrefix {
            address: Ipv4Addr::new(10, 201, 86, 2),
            prefix_len: 24,
        };
        let request = address_change_request(
            RTM_NEWADDR,
            NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL,
            42,
            prefix,
        );

        assert_eq!(read_u32(&request[0..4]).unwrap() as usize, request.len());
        assert_eq!(read_u16(&request[4..6]).unwrap(), RTM_NEWADDR);
        assert_eq!(request[16], AddressFamily::INET.as_raw() as u8);
        assert_eq!(request[17], 24);
        assert_eq!(read_u32(&request[20..24]).unwrap(), 42);
        assert!(request.windows(4).any(|value| value == [10, 201, 86, 2]));
    }

    #[test]
    fn address_parser_prefers_local_ipv4_attribute() {
        let prefix = Ipv4InterfacePrefix {
            address: Ipv4Addr::new(10, 201, 86, 2),
            prefix_len: 24,
        };
        let request = address_change_request(RTM_NEWADDR, 0, 7, prefix);
        let parsed = parse_ipv4_address(&request[16..], 7).unwrap();
        assert_eq!(parsed, Some(prefix));
    }

    #[test]
    fn successful_and_failed_netlink_acknowledgements_are_distinct() {
        assert!(parse_netlink_ack(&0_i32.to_ne_bytes()).is_ok());
        let error = parse_netlink_ack(&(-17_i32).to_ne_bytes()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
    }
}
