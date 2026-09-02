use std::collections::HashMap;

use blueroute_protocol::Command;
use zbus::message::Header;
use zbus::zvariant::Value;
use zbus::{Connection, Proxy, fdo};

pub const MODIFY_ACTION_ID: &str = "org.blueroute.modify";
pub const INTERNET_SHARING_ACTION_ID: &str = "org.blueroute.internet-sharing";

const POLICYKIT_SERVICE: &str = "org.freedesktop.PolicyKit1";
const POLICYKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const POLICYKIT_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const ALLOW_USER_INTERACTION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandAuthorization {
    ReadOnly,
    PolicyKit(&'static str),
}

pub fn command_authorization(command: &Command) -> CommandAuthorization {
    match command {
        Command::GetStatus
        | Command::GetCapabilities
        | Command::ListNetworks
        | Command::ListNodes
        | Command::GetNode { .. }
        | Command::GetDiagnostics => CommandAuthorization::ReadOnly,
        Command::CreateNetwork { .. }
        | Command::JoinNetwork { .. }
        | Command::LeaveNetwork
        | Command::SetDeviceName { .. }
        | Command::StartDiscovery
        | Command::StopDiscovery
        | Command::TrustPeer { .. }
        | Command::ForgetPeer { .. } => CommandAuthorization::PolicyKit(MODIFY_ACTION_ID),
        Command::SetInternetSharing { .. } => {
            CommandAuthorization::PolicyKit(INTERNET_SHARING_ACTION_ID)
        }
    }
}

pub fn command_operation(command: &Command) -> &'static str {
    match command {
        Command::GetStatus => "get_status",
        Command::GetCapabilities => "get_capabilities",
        Command::ListNetworks => "list_networks",
        Command::CreateNetwork { .. } => "create_network",
        Command::JoinNetwork { .. } => "join_network",
        Command::LeaveNetwork => "leave_network",
        Command::ListNodes => "list_nodes",
        Command::GetNode { .. } => "get_node",
        Command::SetDeviceName { .. } => "set_device_name",
        Command::StartDiscovery => "start_discovery",
        Command::StopDiscovery => "stop_discovery",
        Command::TrustPeer { .. } => "trust_peer",
        Command::ForgetPeer { .. } => "forget_peer",
        Command::GetDiagnostics => "get_diagnostics",
        Command::SetInternetSharing { .. } => "set_internet_sharing",
    }
}

pub async fn authorize_command(
    connection: &Connection,
    header: &Header<'_>,
    command: &Command,
) -> fdo::Result<()> {
    let CommandAuthorization::PolicyKit(action_id) = command_authorization(command) else {
        return Ok(());
    };

    let sender = header.sender().ok_or_else(|| {
        fdo::Error::AccessDenied(
            "BlueRoute cannot authorize a privileged request without a D-Bus sender".into(),
        )
    })?;

    let proxy = Proxy::new(
        connection,
        POLICYKIT_SERVICE,
        POLICYKIT_PATH,
        POLICYKIT_INTERFACE,
    )
    .await
    .map_err(|_| authorization_service_failure(command))?;

    let mut subject_details = HashMap::new();
    subject_details.insert("name", Value::new(sender.as_str()));
    let subject = ("system-bus-name", subject_details);
    let details: HashMap<&str, &str> = HashMap::new();

    let result: (bool, bool, HashMap<String, String>) = proxy
        .call(
            "CheckAuthorization",
            &(subject, action_id, details, ALLOW_USER_INTERACTION, ""),
        )
        .await
        .map_err(|_| authorization_service_failure(command))?;

    if result.0 {
        Ok(())
    } else {
        Err(fdo::Error::AccessDenied(format!(
            "authorization denied for BlueRoute operation {}",
            command_operation(command)
        )))
    }
}

fn authorization_service_failure(command: &Command) -> fdo::Error {
    fdo::Error::AccessDenied(format!(
        "authorization unavailable; refusing privileged BlueRoute operation {}",
        command_operation(command)
    ))
}

#[cfg(test)]
mod tests {
    use blueroute_core::{DisplayName, NetworkId, NodeId};

    use super::*;

    #[test]
    fn command_policy_is_explicit_for_every_current_operation() {
        let read_only = [
            Command::GetStatus,
            Command::GetCapabilities,
            Command::ListNetworks,
            Command::ListNodes,
            Command::GetNode {
                node: NodeId::from_bytes([1; 16]),
            },
            Command::GetDiagnostics,
        ];
        for command in read_only {
            assert_eq!(
                command_authorization(&command),
                CommandAuthorization::ReadOnly
            );
        }

        let mutations = [
            Command::CreateNetwork {
                name: DisplayName::new("Authorized network").unwrap(),
            },
            Command::JoinNetwork {
                network: NetworkId::from_bytes([2; 16]),
            },
            Command::LeaveNetwork,
            Command::SetDeviceName {
                name: DisplayName::new("Authorized node").unwrap(),
            },
            Command::StartDiscovery,
            Command::StopDiscovery,
            Command::TrustPeer {
                node: NodeId::from_bytes([3; 16]),
            },
            Command::ForgetPeer {
                node: NodeId::from_bytes([4; 16]),
            },
        ];
        for command in mutations {
            assert_eq!(
                command_authorization(&command),
                CommandAuthorization::PolicyKit(MODIFY_ACTION_ID)
            );
        }

        assert_eq!(
            command_authorization(&Command::SetInternetSharing { enabled: true }),
            CommandAuthorization::PolicyKit(INTERNET_SHARING_ACTION_ID)
        );
    }
}
