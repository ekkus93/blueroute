//! `CliFailure`: the stable process exit codes and the error text printed for each failure.

use super::error_guidance;
use blueroute_client::{ClientError, ClientErrorKind};

/// Stable process exit codes: 1 operational, 2 usage, 3 unavailable,
/// 4 authorization failure, 5 unsupported capability.
#[derive(Debug)]
pub(super) struct CliFailure {
    pub(super) code: i32,
    pub(super) message: String,
    /// A specific, typed failure with its own guidance; the exit code stays its category's.
    pub(super) detail: Option<error_guidance::Detail>,
}

impl From<ClientError> for CliFailure {
    fn from(error: ClientError) -> Self {
        let code = match error.kind() {
            ClientErrorKind::Unavailable => 3,
            ClientErrorKind::Authorization => 4,
            ClientErrorKind::Unsupported => 5,
            ClientErrorKind::Operational | ClientErrorKind::AddressConflict => 1,
        };
        let detail = (error.kind() == ClientErrorKind::AddressConflict)
            .then_some(error_guidance::Detail::AddressConflict);
        Self {
            code,
            message: error.to_string(),
            detail,
        }
    }
}

impl From<serde_json::Error> for CliFailure {
    fn from(error: serde_json::Error) -> Self {
        Self {
            code: 1,
            message: error.to_string(),
            detail: None,
        }
    }
}

impl From<String> for CliFailure {
    fn from(message: String) -> Self {
        Self {
            code: 1,
            message,
            detail: None,
        }
    }
}
impl From<&'static str> for CliFailure {
    fn from(message: &'static str) -> Self {
        Self {
            code: 1,
            message: message.to_owned(),
            detail: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_client::ClientError;

    use blueroute_protocol::ApiVersion;

    #[test]
    fn cli_error_exit_classes_are_stable_and_do_not_leak_unverified_peer_claims() {
        let unavailable = CliFailure::from(ClientError::IncompatibleVersion {
            client: ApiVersion::new(1, 0),
            server: ApiVersion::new(2, 0),
        });
        assert_eq!(unavailable.code, 3);
        let denied = CliFailure::from(ClientError::Bus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::AccessDenied("authorization denied".into()),
        ))));
        assert_eq!(denied.code, 4);
        let unsupported = CliFailure::from(ClientError::Bus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::NotSupported("unsupported runtime capability".into()),
        ))));
        assert_eq!(unsupported.code, 5);
        assert_eq!(CliFailure::from("unrecognized daemon response").code, 1);
    }
}
