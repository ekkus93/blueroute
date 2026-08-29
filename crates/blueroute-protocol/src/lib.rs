#![doc = "Shared BlueRoute protocol and API types."]

use std::fmt;

/// Version of the local daemon API contract.
///
/// Major versions are incompatible. Minor versions are additive: a server can
/// serve a client with the same major version and a minor version no newer than
/// the server's own minor version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    pub const CURRENT: Self = Self { major: 1, minor: 0 };

    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    pub const fn compatibility_with_server(self, server: Self) -> ApiCompatibility {
        if self.major != server.major || self.minor > server.minor {
            ApiCompatibility::Incompatible
        } else if self.minor == server.minor {
            ApiCompatibility::Exact
        } else {
            ApiCompatibility::Compatible
        }
    }

    pub const fn is_compatible_with_server(self, server: Self) -> bool {
        !matches!(
            self.compatibility_with_server(server),
            ApiCompatibility::Incompatible
        )
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiCompatibility {
    Exact,
    Compatible,
    Incompatible,
}

/// Well-known system D-Bus service name for the v1 daemon API.
pub const DBUS_SERVICE_NAME: &str = "org.blueroute.Service1";
/// Well-known root object path for the v1 daemon API.
pub const DBUS_OBJECT_PATH: &str = "/org/blueroute/Service1";
/// Well-known D-Bus interface for the v1 daemon API.
pub const DBUS_INTERFACE_NAME: &str = "org.blueroute.Service1";

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    #[test]
    fn exact_api_versions_are_compatible() {
        assert_eq!(
            ApiVersion::CURRENT.compatibility_with_server(ApiVersion::CURRENT),
            ApiCompatibility::Exact
        );
    }

    #[test]
    fn older_minor_client_is_compatible_with_newer_server() {
        let client = ApiVersion::new(1, 2);
        let server = ApiVersion::new(1, 5);
        assert_eq!(
            client.compatibility_with_server(server),
            ApiCompatibility::Compatible
        );
        assert!(client.is_compatible_with_server(server));
    }

    #[test]
    fn newer_minor_client_is_rejected_by_older_server() {
        let client = ApiVersion::new(1, 5);
        let server = ApiVersion::new(1, 2);
        assert_eq!(
            client.compatibility_with_server(server),
            ApiCompatibility::Incompatible
        );
    }

    #[test]
    fn different_major_versions_are_incompatible() {
        assert!(!ApiVersion::new(1, 9).is_compatible_with_server(ApiVersion::new(2, 0)));
    }

    #[test]
    fn display_is_stable() {
        assert_eq!(ApiVersion::new(12, 34).to_string(), "12.34");
    }

    #[test]
    fn ordering_is_lexicographic_by_major_then_minor() {
        assert_eq!(
            ApiVersion::new(1, 9).cmp(&ApiVersion::new(2, 0)),
            Ordering::Less
        );
    }

    #[test]
    fn dbus_names_are_versioned() {
        assert!(DBUS_SERVICE_NAME.ends_with("Service1"));
        assert!(DBUS_OBJECT_PATH.ends_with("Service1"));
        assert!(DBUS_INTERFACE_NAME.ends_with("Service1"));
    }
}
