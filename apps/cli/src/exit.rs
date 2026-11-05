//! Exit-code contract (P10-006). Scripts may rely on these values.

use blueroute_client::ClientError;
use zbus::fdo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Exit {
    Success = 0,
    /// The daemon accepted the request but the operation failed.
    OperationFailed = 1,
    /// Bad command line or invalid argument value.
    Usage = 2,
    /// The daemon (or the system bus) could not be reached.
    DaemonUnavailable = 3,
    /// This node cannot do what was asked (missing adapter, capability, feature).
    Unsupported = 4,
    /// PolicyKit/daemon authorization refused the action.
    Unauthorized = 5,
}

impl Exit {
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// Classify a client error into the exit contract.
pub fn classify(error: &ClientError) -> Exit {
    match error {
        ClientError::Bus(error) => classify_bus(error),
        ClientError::ReconnectTimeout { .. } | ClientError::EventStreamClosed => {
            Exit::DaemonUnavailable
        }
        ClientError::IncompatibleVersion { .. }
        | ClientError::Protocol(_)
        | ClientError::UnexpectedResponse { .. }
        | ClientError::InvalidTransportPayload(_) => Exit::OperationFailed,
    }
}

fn classify_bus(error: &zbus::Error) -> Exit {
    match error {
        zbus::Error::FDO(inner) => match **inner {
            fdo::Error::AccessDenied(_) | fdo::Error::AuthFailed(_) => Exit::Unauthorized,
            fdo::Error::NotSupported(_) | fdo::Error::UnknownMethod(_) => Exit::Unsupported,
            fdo::Error::InvalidArgs(_) => Exit::Usage,
            fdo::Error::ServiceUnknown(_)
            | fdo::Error::NameHasNoOwner(_)
            | fdo::Error::NoServer(_)
            | fdo::Error::Disconnected(_)
            | fdo::Error::TimedOut(_)
            | fdo::Error::IOError(_) => Exit::DaemonUnavailable,
            _ => Exit::OperationFailed,
        },
        zbus::Error::InputOutput(_)
        | zbus::Error::Address(_)
        | zbus::Error::Connection(..)
        | zbus::Error::Handshake(_) => Exit::DaemonUnavailable,
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "org.freedesktop.DBus.Error.AccessDenied" => Exit::Unauthorized,
            "org.freedesktop.DBus.Error.NotSupported" => Exit::Unsupported,
            "org.freedesktop.DBus.Error.InvalidArgs" => Exit::Usage,
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => Exit::DaemonUnavailable,
            _ => Exit::OperationFailed,
        },
        _ => Exit::OperationFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus(error: fdo::Error) -> ClientError {
        ClientError::Bus(zbus::Error::FDO(Box::new(error)))
    }

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            [
                Exit::Success,
                Exit::OperationFailed,
                Exit::Usage,
                Exit::DaemonUnavailable,
                Exit::Unsupported,
                Exit::Unauthorized
            ]
            .map(Exit::code),
            [0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn daemon_errors_map_to_the_contract() {
        let x = || "x".to_owned();
        assert_eq!(
            classify(&bus(fdo::Error::AccessDenied(x()))),
            Exit::Unauthorized
        );
        assert_eq!(
            classify(&bus(fdo::Error::NotSupported(x()))),
            Exit::Unsupported
        );
        assert_eq!(classify(&bus(fdo::Error::InvalidArgs(x()))), Exit::Usage);
        assert_eq!(
            classify(&bus(fdo::Error::ServiceUnknown(x()))),
            Exit::DaemonUnavailable
        );
        assert_eq!(
            classify(&bus(fdo::Error::NameHasNoOwner(x()))),
            Exit::DaemonUnavailable
        );
        assert_eq!(
            classify(&bus(fdo::Error::Failed(x()))),
            Exit::OperationFailed
        );
    }

    #[test]
    fn client_side_failures_map_sensibly() {
        assert_eq!(
            classify(&ClientError::ReconnectTimeout {
                timeout: std::time::Duration::from_secs(1),
                last_error: String::new()
            }),
            Exit::DaemonUnavailable
        );
        assert_eq!(
            classify(&ClientError::UnexpectedResponse {
                expected: "status",
                actual: "ack"
            }),
            Exit::OperationFailed
        );
    }
}
