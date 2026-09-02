use std::fs;
use std::path::Path;
use std::process::Command;

use blueroute_core::{
    CapabilitySource, CoreError, DaemonConfig, NetworkBackend, NodeCapabilities, Sourced,
};
use zbus::fdo::ObjectManagerProxy;

use crate::{
    BackendFuture, BluetoothBackend, BluezBackend, CapabilityProbe, NetworkManagerBackend,
    RuntimeCapabilities,
};

const BLUEZ_SERVICE: &str = "org.bluez";
const BLUEZ_ROOT_PATH: &str = "/";
const NETWORK_SERVER_INTERFACE: &str = "org.bluez.NetworkServer1";
const BNEP_MODULE_PATH: &str = "/sys/module/bnep";
const BLUETOOTH_SYSFS_PATH: &str = "/sys/class/bluetooth";
const IPV4_FORWARD_PATH: &str = "/proc/sys/net/ipv4/ip_forward";
const KERNEL_RELEASE_PATH: &str = "/proc/sys/kernel/osrelease";
const CONSERVATIVE_ACTIVE_PEER_CEILING: u16 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemSupportLevel {
    FullySupported,
    ClientOnly,
    Degraded,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityDiagnosticLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityDiagnostic {
    pub level: CapabilityDiagnosticLevel,
    pub component: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothControllerReport {
    pub handle: String,
    pub powered: bool,
    pub address: Option<String>,
    pub driver: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimePrerequisite {
    pub name: String,
    pub available: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemCapabilityReport {
    pub support: SystemSupportLevel,
    pub runtime: RuntimeCapabilities,
    pub bluez_version: Option<String>,
    pub network_backend_version: Option<String>,
    pub controllers: Vec<BluetoothControllerReport>,
    pub panu_available: bool,
    pub nap_available: Option<bool>,
    pub forwarding_available: bool,
    pub forwarding_enabled: Option<bool>,
    pub practical_peer_ceiling: u16,
    pub configured_peer_ceiling: Option<u16>,
    pub effective_peer_ceiling: u16,
    pub kernel_release: Option<String>,
    pub prerequisites: Vec<RuntimePrerequisite>,
    pub diagnostics: Vec<CapabilityDiagnostic>,
}

#[derive(Clone, Debug, Default)]
pub struct SystemCapabilityProbe {
    config: DaemonConfig,
}

#[derive(Clone, Copy, Debug)]
struct SupportInputs {
    bluez_available: bool,
    network_backend_available: bool,
    adapter_present: bool,
    powered_adapter: bool,
    bnep_available: bool,
    panu_available: bool,
    nap_available: Option<bool>,
    forwarding_available: bool,
}

impl SystemCapabilityProbe {
    pub fn new(config: DaemonConfig) -> Result<Self, CoreError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub async fn report(&self) -> Result<SystemCapabilityReport, CoreError> {
        self.config.validate()?;
        let mut diagnostics = Vec::new();

        let bluez_version = resolve_bluez_version();
        if bluez_version.is_none() {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Warning,
                component: "bluez".into(),
                message: "BlueZ daemon version could not be determined from known bluetoothd locations or PATH".into(),
            });
        }

        let bluez = match BluezBackend::connect_system().await {
            Ok(backend) => Some(backend),
            Err(error) => {
                diagnostics.push(CapabilityDiagnostic {
                    level: CapabilityDiagnosticLevel::Error,
                    component: "bluez".into(),
                    message: format!("BlueZ system service is unavailable: {error}"),
                });
                None
            }
        };
        let bluez_available = bluez.is_some();

        let adapters = if let Some(backend) = &bluez {
            match backend.adapters().await {
                Ok(adapters) => adapters,
                Err(error) => {
                    diagnostics.push(CapabilityDiagnostic {
                        level: CapabilityDiagnosticLevel::Error,
                        component: "bluetooth".into(),
                        message: format!("Bluetooth adapters could not be enumerated: {error}"),
                    });
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let controllers = adapters
            .iter()
            .map(|adapter| controller_report(adapter.handle.as_str(), adapter.powered))
            .collect::<Vec<_>>();
        let powered_adapter = adapters.iter().any(|adapter| adapter.powered);
        if adapters.is_empty() && bluez_available {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Error,
                component: "bluetooth".into(),
                message: "BlueZ is running but no Bluetooth adapter is exposed".into(),
            });
        } else if !adapters.is_empty() && !powered_adapter {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Warning,
                component: "bluetooth".into(),
                message: "Bluetooth adapter is present but no adapter is powered".into(),
            });
        }

        let bnep_available = Path::new(BNEP_MODULE_PATH).exists();
        if !bnep_available {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Error,
                component: "bluetooth-pan".into(),
                message: "Linux BNEP runtime support is not present at /sys/module/bnep".into(),
            });
        }

        let nap_available = match &bluez {
            Some(backend) => match has_bluez_interface(backend, NETWORK_SERVER_INTERFACE).await {
                Ok(value) => Some(value && bnep_available && powered_adapter),
                Err(error) => {
                    diagnostics.push(CapabilityDiagnostic {
                        level: CapabilityDiagnosticLevel::Warning,
                        component: "nap".into(),
                        message: format!("NAP capability could not be determined from BlueZ managed objects: {error}"),
                    });
                    None
                }
            },
            None => Some(false),
        };
        if nap_available == Some(false) && bluez_available && powered_adapter {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Info,
                component: "nap".into(),
                message: "local NAP server capability is unavailable; this node may still operate as a PANU client".into(),
            });
        }

        let panu_available = bluez_available && powered_adapter && bnep_available;
        if panu_available {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Info,
                component: "panu".into(),
                message: "PANU prerequisites are present; remote peer compatibility is evaluated when connecting".into(),
            });
        }

        let network_manager = match NetworkManagerBackend::connect_system().await {
            Ok(backend) => Some(backend),
            Err(error) => {
                diagnostics.push(CapabilityDiagnostic {
                    level: CapabilityDiagnosticLevel::Error,
                    component: "network-backend".into(),
                    message: format!("NetworkManager system service is unavailable: {error}"),
                });
                None
            }
        };
        let network_backend_version = match &network_manager {
            Some(backend) => match backend.version().await {
                Ok(version) => Some(version),
                Err(error) => {
                    diagnostics.push(CapabilityDiagnostic {
                        level: CapabilityDiagnosticLevel::Warning,
                        component: "network-backend".into(),
                        message: format!("NetworkManager version could not be queried: {error}"),
                    });
                    None
                }
            },
            None => None,
        };

        let (forwarding_available, forwarding_enabled) = inspect_forwarding();
        if !forwarding_available {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Warning,
                component: "forwarding".into(),
                message: "kernel IPv4 forwarding control is unavailable or unreadable".into(),
            });
        }

        let practical_peer_ceiling = CONSERVATIVE_ACTIVE_PEER_CEILING;
        let configured_peer_ceiling = self.config.topology.max_active_links;
        let effective_peer_ceiling = effective_peer_ceiling(configured_peer_ceiling);
        if configured_peer_ceiling.is_some_and(|value| value > practical_peer_ceiling) {
            diagnostics.push(CapabilityDiagnostic {
                level: CapabilityDiagnosticLevel::Warning,
                component: "peer-ceiling".into(),
                message: format!(
                    "configured active-link ceiling exceeds the conservative runtime ceiling of {practical_peer_ceiling}; the effective ceiling is {effective_peer_ceiling}"
                ),
            });
        }

        let kernel_release = read_trimmed(Path::new(KERNEL_RELEASE_PATH));
        let prerequisites = vec![
            RuntimePrerequisite {
                name: "bluetooth-sysfs".into(),
                available: Path::new(BLUETOOTH_SYSFS_PATH).exists(),
                detail: BLUETOOTH_SYSFS_PATH.into(),
            },
            RuntimePrerequisite {
                name: "bnep".into(),
                available: bnep_available,
                detail: BNEP_MODULE_PATH.into(),
            },
            RuntimePrerequisite {
                name: "ipv4-forwarding-control".into(),
                available: forwarding_available,
                detail: IPV4_FORWARD_PATH.into(),
            },
            RuntimePrerequisite {
                name: "bluez-system-service".into(),
                available: bluez_available,
                detail: BLUEZ_SERVICE.into(),
            },
            RuntimePrerequisite {
                name: "networkmanager-system-service".into(),
                available: network_manager.is_some(),
                detail: "org.freedesktop.NetworkManager".into(),
            },
        ];

        let mut node = NodeCapabilities {
            adapter_usable: Some(Sourced::new(powered_adapter, CapabilitySource::Discovered)),
            panu: Some(Sourced::new(panu_available, CapabilitySource::Discovered)),
            nap: nap_available.map(|value| Sourced::new(value, CapabilitySource::Discovered)),
            routing: Some(Sourced::new(
                forwarding_available,
                CapabilitySource::Discovered,
            )),
            connection_policy_ceiling: Some(Sourced::new(
                effective_peer_ceiling,
                match configured_peer_ceiling {
                    Some(value) if value <= practical_peer_ceiling => CapabilitySource::Configured,
                    _ => CapabilitySource::ConservativeDefault,
                },
            )),
            ..NodeCapabilities::default()
        };
        if network_manager.is_some() {
            node.network_backend = Some(Sourced::new(
                NetworkBackend::NetworkManager,
                CapabilitySource::Discovered,
            ));
        }

        let runtime = RuntimeCapabilities {
            bluez_available,
            network_backend: network_manager
                .as_ref()
                .map(|_| NetworkBackend::NetworkManager),
            node,
        };

        let support = classify_support(SupportInputs {
            bluez_available,
            network_backend_available: network_manager.is_some(),
            adapter_present: !controllers.is_empty(),
            powered_adapter,
            bnep_available,
            panu_available,
            nap_available,
            forwarding_available,
        });
        diagnostics.insert(
            0,
            CapabilityDiagnostic {
                level: match support {
                    SystemSupportLevel::Unsupported => CapabilityDiagnosticLevel::Error,
                    SystemSupportLevel::Degraded => CapabilityDiagnosticLevel::Warning,
                    SystemSupportLevel::ClientOnly | SystemSupportLevel::FullySupported => {
                        CapabilityDiagnosticLevel::Info
                    }
                },
                component: "summary".into(),
                message: support_message(support).into(),
            },
        );

        Ok(SystemCapabilityReport {
            support,
            runtime,
            bluez_version,
            network_backend_version,
            controllers,
            panu_available,
            nap_available,
            forwarding_available,
            forwarding_enabled,
            practical_peer_ceiling,
            configured_peer_ceiling,
            effective_peer_ceiling,
            kernel_release,
            prerequisites,
            diagnostics,
        })
    }
}

impl CapabilityProbe for SystemCapabilityProbe {
    fn probe(&self) -> BackendFuture<'_, RuntimeCapabilities> {
        Box::pin(async move { Ok(self.report().await?.runtime) })
    }
}

async fn has_bluez_interface(
    backend: &BluezBackend,
    interface_name: &str,
) -> Result<bool, CoreError> {
    let proxy = ObjectManagerProxy::new(&backend.connection, BLUEZ_SERVICE, BLUEZ_ROOT_PATH)
        .await
        .map_err(|error| {
            CoreError::with_diagnostic(
                blueroute_core::ErrorKind::BluezUnavailable,
                "failed to inspect BlueZ managed objects for capability reporting",
                error.to_string(),
            )
        })?;
    let objects = proxy.get_managed_objects().await.map_err(|error| {
        CoreError::with_diagnostic(
            blueroute_core::ErrorKind::BluezUnavailable,
            "failed to enumerate BlueZ managed objects for capability reporting",
            error.to_string(),
        )
    })?;
    Ok(objects.values().any(|interfaces| {
        interfaces
            .keys()
            .any(|interface| interface.as_str() == interface_name)
    }))
}

fn controller_report(handle: &str, powered: bool) -> BluetoothControllerReport {
    let name = handle.rsplit('/').next().filter(|value| !value.is_empty());
    let (address, driver) = match name {
        Some(name) => {
            let base = Path::new(BLUETOOTH_SYSFS_PATH).join(name);
            let address = read_trimmed(&base.join("address"));
            let driver = fs::read_link(base.join("device/driver"))
                .ok()
                .and_then(|path| {
                    path.file_name()
                        .map(|value| value.to_string_lossy().into_owned())
                });
            (address, driver)
        }
        None => (None, None),
    };
    BluetoothControllerReport {
        handle: handle.into(),
        powered,
        address,
        driver,
    }
}

fn inspect_forwarding() -> (bool, Option<bool>) {
    match fs::read_to_string(IPV4_FORWARD_PATH) {
        Ok(value) => match value.trim() {
            "0" => (true, Some(false)),
            "1" => (true, Some(true)),
            _ => (false, None),
        },
        Err(_) => (false, None),
    }
}

fn resolve_bluez_version() -> Option<String> {
    let candidates = [
        "/usr/libexec/bluetooth/bluetoothd",
        "/usr/lib/bluetooth/bluetoothd",
        "/usr/sbin/bluetoothd",
        "bluetoothd",
    ];
    for candidate in candidates {
        let output = Command::new(candidate).arg("--version").output();
        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !version.is_empty() {
            return Some(version);
        }
    }
    None
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn effective_peer_ceiling(configured: Option<u16>) -> u16 {
    configured
        .unwrap_or(CONSERVATIVE_ACTIVE_PEER_CEILING)
        .min(CONSERVATIVE_ACTIVE_PEER_CEILING)
}

fn classify_support(input: SupportInputs) -> SystemSupportLevel {
    if !input.bluez_available
        || !input.network_backend_available
        || !input.adapter_present
        || !input.bnep_available
    {
        return SystemSupportLevel::Unsupported;
    }
    if !input.powered_adapter || input.nap_available.is_none() {
        return SystemSupportLevel::Degraded;
    }
    if input.panu_available && input.nap_available == Some(false) {
        return SystemSupportLevel::ClientOnly;
    }
    if input.panu_available && input.nap_available == Some(true) && input.forwarding_available {
        return SystemSupportLevel::FullySupported;
    }
    SystemSupportLevel::Degraded
}

fn support_message(level: SystemSupportLevel) -> &'static str {
    match level {
        SystemSupportLevel::FullySupported => {
            "system satisfies current BlueRoute PANU, NAP, network-backend, and forwarding prerequisites"
        }
        SystemSupportLevel::ClientOnly => {
            "system can operate as a PANU client but local NAP hosting is unavailable"
        }
        SystemSupportLevel::Degraded => {
            "system has partial BlueRoute support; diagnostics identify the missing or indeterminate capability"
        }
        SystemSupportLevel::Unsupported => {
            "system is missing a required BlueRoute runtime service, Bluetooth adapter, or BNEP prerequisite"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn support_inputs() -> SupportInputs {
        SupportInputs {
            bluez_available: true,
            network_backend_available: true,
            adapter_present: true,
            powered_adapter: true,
            bnep_available: true,
            panu_available: true,
            nap_available: Some(true),
            forwarding_available: true,
        }
    }

    #[test]
    fn classification_distinguishes_supported_modes() {
        assert_eq!(
            classify_support(support_inputs()),
            SystemSupportLevel::FullySupported
        );
        assert_eq!(
            classify_support(SupportInputs {
                nap_available: Some(false),
                ..support_inputs()
            }),
            SystemSupportLevel::ClientOnly
        );
        assert_eq!(
            classify_support(SupportInputs {
                powered_adapter: false,
                panu_available: false,
                nap_available: Some(false),
                ..support_inputs()
            }),
            SystemSupportLevel::Degraded
        );
        assert_eq!(
            classify_support(SupportInputs {
                bluez_available: false,
                ..support_inputs()
            }),
            SystemSupportLevel::Unsupported
        );
    }

    #[test]
    fn configured_peer_ceiling_is_capped_by_conservative_runtime_limit() {
        assert_eq!(effective_peer_ceiling(None), 4);
        assert_eq!(effective_peer_ceiling(Some(2)), 2);
        assert_eq!(effective_peer_ceiling(Some(4)), 4);
        assert_eq!(effective_peer_ceiling(Some(9)), 4);
    }

    #[test]
    fn controller_name_is_derived_only_from_bluez_object_path() {
        let report = controller_report("/org/bluez/hci999999", true);
        assert_eq!(report.handle, "/org/bluez/hci999999");
        assert!(report.address.is_none());
        assert!(report.driver.is_none());
    }
}
