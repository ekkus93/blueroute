use std::error::Error;
use std::fmt;

/// High-level error categories exposed by the hardware-independent core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    UnsupportedRuntime,
    MissingAdapter,
    AdapterDisabled,
    CapabilityUnavailable,
    BluezUnavailable,
    NetworkBackendUnavailable,
    PairingFailed,
    AuthenticationFailed,
    PanFailure,
    AddressConflict,
    RouteFailure,
    TopologyFailure,
    ProtocolError,
    InvalidState,
    InvalidInput,
    PersistenceError,
    Internal,
}

impl ErrorKind {
    /// Whether another bounded transport attempt may help without changing
    /// configuration or credentials. Authentication and protocol failures are
    /// deliberately permanent for the current peer/session generation.
    pub const fn is_retryable_transport_failure(self) -> bool {
        matches!(
            self,
            Self::BluezUnavailable | Self::NetworkBackendUnavailable | Self::PanFailure
        )
    }
}

/// A typed core error with a friendly message and optional diagnostic context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoreError {
    kind: ErrorKind,
    message: String,
    diagnostic: Option<String>,
}

impl CoreError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(
        kind: ErrorKind,
        message: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostic: Some(diagnostic.into()),
        }
    }

    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_message_is_separate_from_diagnostic_context() {
        let error = CoreError::with_diagnostic(
            ErrorKind::BluezUnavailable,
            "Bluetooth service is unavailable",
            "org.freedesktop.DBus.Error.ServiceUnknown",
        );

        assert_eq!(error.to_string(), "Bluetooth service is unavailable");
        assert_eq!(error.kind(), ErrorKind::BluezUnavailable);
        assert_eq!(
            error.diagnostic(),
            Some("org.freedesktop.DBus.Error.ServiceUnknown")
        );
    }

    #[test]
    fn only_transient_transport_errors_are_retryable() {
        for kind in [
            ErrorKind::BluezUnavailable,
            ErrorKind::NetworkBackendUnavailable,
            ErrorKind::PanFailure,
        ] {
            assert!(kind.is_retryable_transport_failure());
        }
        for kind in [
            ErrorKind::UnsupportedRuntime,
            ErrorKind::MissingAdapter,
            ErrorKind::AdapterDisabled,
            ErrorKind::CapabilityUnavailable,
            ErrorKind::PairingFailed,
            ErrorKind::AuthenticationFailed,
            ErrorKind::AddressConflict,
            ErrorKind::RouteFailure,
            ErrorKind::TopologyFailure,
            ErrorKind::ProtocolError,
            ErrorKind::InvalidState,
            ErrorKind::InvalidInput,
            ErrorKind::PersistenceError,
            ErrorKind::Internal,
        ] {
            assert!(!kind.is_retryable_transport_failure(), "{kind:?}");
        }
    }
}
