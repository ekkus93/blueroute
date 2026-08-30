from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


path = Path("crates/blueroute-linux/src/pan.rs")
text = path.read_text()

text = replace_once(
    text,
    "use std::collections::HashMap;\nuse std::time::Duration;",
    "use std::collections::{HashMap, HashSet};\nuse std::fs;\nuse std::time::Duration;",
    "imports",
)

text = replace_once(
    text,
    'const BLUEZ_SERVICE: &str = "org.bluez";\nconst NETWORK_INTERFACE: &str = "org.bluez.Network1";',
    'const BLUEZ_SERVICE: &str = "org.bluez";\nconst ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";\nconst NETWORK_INTERFACE: &str = "org.bluez.Network1";',
    "adapter interface constant",
)

text = replace_once(
    text,
    'const INTERFACE_PROPERTY: &str = "Interface";\nconst REMOTE_NAP_ROLE: &str = "nap";\nconst SIGNAL_QUEUE_CAPACITY: usize = 64;\nconst PANU_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);',
    'const INTERFACE_PROPERTY: &str = "Interface";\nconst ADDRESS_PROPERTY: &str = "Address";\nconst REMOTE_NAP_ROLE: &str = "nap";\nconst SIGNAL_QUEUE_CAPACITY: usize = 64;\nconst PANU_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);\nconst PANU_INTERFACE_SETTLE_DELAY: Duration = Duration::from_millis(300);\nconst SYS_CLASS_NET: &str = "/sys/class/net";',
    "interface resolution constants",
)

text = replace_once(
    text,
    '''    let proxy = network_proxy(connection, peer).await?;\n    let connect = async {''',
    '''    let local_address = local_adapter_address_for_peer(connection, peer).await?;\n    let previous_interfaces = kernel_interfaces_with_address(&local_address)?;\n    let proxy = network_proxy(connection, peer).await?;\n    let connect = async {''',
    "capture pre-connect interfaces",
)

text = replace_once(
    text,
    '''            })?;\n            panu_attachment(peer, interface)\n        }\n        Ok(None) => match abort_pending_panu_connect(connection, peer).await {''',
    '''            })?;\n            // BlueZ returns the kernel-created name (normally bnepN), but udev may\n            // immediately rename that netdev according to predictable-interface policy.\n            // Give udev a short settle window, then resolve the live kernel name.\n            Timer::after(PANU_INTERFACE_SETTLE_DELAY).await;\n            let current_interfaces = kernel_interfaces_with_address(&local_address)?;\n            let interface = select_panu_interface_name(\n                &interface,\n                &current_interfaces,\n                Some(&previous_interfaces),\n            )?;\n            panu_attachment(peer, interface)\n        }\n        Ok(None) => match abort_pending_panu_connect(connection, peer).await {''',
    "resolve successful connect interface",
)

text = replace_once(
    text,
    '''    let interface: String = proxy\n        .get_property(INTERFACE_PROPERTY)\n        .await\n        .map_err(|error| property_error(INTERFACE_PROPERTY, error))?;\n    panu_attachment(peer, interface).map(Some)\n}\n\nasync fn network_proxy<'a>(''',
    '''    let interface: String = proxy\n        .get_property(INTERFACE_PROPERTY)\n        .await\n        .map_err(|error| property_error(INTERFACE_PROPERTY, error))?;\n    let local_address = local_adapter_address_for_peer(connection, peer).await?;\n    let current_interfaces = kernel_interfaces_with_address(&local_address)?;\n    let interface = select_panu_interface_name(&interface, &current_interfaces, None)?;\n    panu_attachment(peer, interface).map(Some)\n}\n\nasync fn local_adapter_address_for_peer(\n    connection: &Connection,\n    peer: &PeerHandle,\n) -> Result<String, CoreError> {\n    let adapter_path = peer_adapter_path(peer)?;\n    let proxy = Proxy::new(connection, BLUEZ_SERVICE, adapter_path, ADAPTER_INTERFACE)\n        .await\n        .map_err(|error| {\n            pan_error(\n                ErrorKind::BluezUnavailable,\n                "failed to access the Bluetooth adapter for PAN interface resolution",\n                error,\n            )\n        })?;\n    proxy\n        .get_property(ADDRESS_PROPERTY)\n        .await\n        .map_err(|error| property_error(ADDRESS_PROPERTY, error))\n}\n\nfn peer_adapter_path(peer: &PeerHandle) -> Result<&str, CoreError> {\n    peer.as_str()\n        .rsplit_once("/dev_")\n        .map(|(adapter, _)| adapter)\n        .filter(|adapter| !adapter.is_empty())\n        .ok_or_else(|| {\n            CoreError::new(\n                ErrorKind::ProtocolError,\n                "Bluetooth peer handle does not identify its adapter",\n            )\n        })\n}\n\nfn kernel_interfaces_with_address(address: &str) -> Result<HashSet<String>, CoreError> {\n    let entries = fs::read_dir(SYS_CLASS_NET).map_err(|error| {\n        pan_error(\n            ErrorKind::PanFailure,\n            "failed to inspect Linux network interfaces for the Bluetooth PAN link",\n            error,\n        )\n    })?;\n    let mut interfaces = HashSet::new();\n    for entry in entries {\n        let entry = entry.map_err(|error| {\n            pan_error(\n                ErrorKind::PanFailure,\n                "failed to inspect a Linux network interface for the Bluetooth PAN link",\n                error,\n            )\n        })?;\n        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {\n            continue;\n        };\n        let interface_address = match fs::read_to_string(entry.path().join("address")) {\n            Ok(value) => value,\n            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,\n            Err(error) => {\n                return Err(pan_error(\n                    ErrorKind::PanFailure,\n                    "failed to read a Linux network-interface address for PAN resolution",\n                    error,\n                ));\n            }\n        };\n        if interface_address.trim().eq_ignore_ascii_case(address) {\n            interfaces.insert(name);\n        }\n    }\n    Ok(interfaces)\n}\n\nfn select_panu_interface_name(\n    reported: &str,\n    current: &HashSet<String>,\n    previous: Option<&HashSet<String>>,\n) -> Result<String, CoreError> {\n    if current.contains(reported) {\n        return Ok(reported.to_owned());\n    }\n\n    if let Some(previous) = previous {\n        let new_interfaces = current\n            .difference(previous)\n            .cloned()\n            .collect::<Vec<String>>();\n        if new_interfaces.len() == 1 {\n            return Ok(new_interfaces[0].clone());\n        }\n    } else if current.len() == 1 {\n        return Ok(current.iter().next().expect("one interface exists").clone());\n    }\n\n    let mut candidates = current.iter().cloned().collect::<Vec<_>>();\n    candidates.sort();\n    Err(CoreError::with_diagnostic(\n        ErrorKind::PanFailure,\n        "Bluetooth PAN interface was renamed but its current Linux name could not be resolved",\n        format!(\n            "BlueZ reported {reported:?}; matching kernel interfaces: {candidates:?}"\n        ),\n    ))\n}\n\nasync fn network_proxy<'a>(''',
    "kernel interface rename resolution helpers",
)

insert_at = text.rfind("\n}")
if insert_at < 0:
    raise SystemExit("tests module closing brace not found")
tests = r'''

    #[test]
    fn reported_kernel_interface_is_preferred_when_it_still_exists() {
        let current = HashSet::from(["bnep0".to_owned(), "enx001122334455".to_owned()]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, None).unwrap(),
            "bnep0"
        );
    }

    #[test]
    fn newly_renamed_interface_is_selected_after_connect() {
        let previous = HashSet::from(["enxaaaaaaaaaaaa".to_owned()]);
        let current = HashSet::from([
            "enxaaaaaaaaaaaa".to_owned(),
            "enx001122334455".to_owned(),
        ]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, Some(&previous)).unwrap(),
            "enx001122334455"
        );
    }

    #[test]
    fn existing_connection_resolves_unique_renamed_interface() {
        let current = HashSet::from(["enx001122334455".to_owned()]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, None).unwrap(),
            "enx001122334455"
        );
    }

    #[test]
    fn ambiguous_renamed_interfaces_fail_instead_of_guessing() {
        let current = HashSet::from([
            "enx001122334455".to_owned(),
            "enx001122334456".to_owned(),
        ]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, None)
                .unwrap_err()
                .kind(),
            ErrorKind::PanFailure
        );
    }
'''
text = text[:insert_at] + tests + text[insert_at:]
path.write_text(text)

# Document the observed real-hardware rename behavior.
doc = Path("docs/P4-005-PANU.md")
doc_text = doc.read_text()
needle = "Before starting a new connection, BlueRoute reconciles the authoritative `Network1.Connected` and `Network1.Interface` properties and returns an existing attachment when the requested state is already satisfied."
replacement = needle + " BlueZ may continue to report the original `bnepN` name after systemd-udevd applies a predictable-interface rename (observed on hardware as `bnep0` -> `enx...`). BlueRoute therefore reconciles the reported name against `/sys/class/net` using the local Bluetooth adapter address and, for a fresh connection, the pre-connect interface snapshot; ambiguous matches fail explicitly instead of guessing."
if doc_text.count(needle) != 1:
    raise SystemExit("documentation connection paragraph match failed")
doc.write_text(doc_text.replace(needle, replacement, 1))

todo = Path("docs/TODO.md")
todo_text = todo.read_text()
old = "BlueZ Network1 PANU connect/interface mapping, bounded connect timeout/cancellation, loss observation, and idempotent disconnect are implemented; working PANU data-plane hardware acceptance remains pending."
new = "BlueZ Network1 PANU connect/interface mapping (including udev rename reconciliation), bounded connect timeout/cancellation, loss observation, and idempotent disconnect are implemented; working PANU data-plane hardware acceptance remains pending."
if todo_text.count(old) != 1:
    raise SystemExit("TODO P4-005 summary match failed")
todo.write_text(todo_text.replace(old, new, 1))
