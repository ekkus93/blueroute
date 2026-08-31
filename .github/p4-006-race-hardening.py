from pathlib import Path

path = Path("crates/blueroute-linux/src/pan.rs")
text = path.read_text()


def replace_exact(old: str, new: str, expected: int = 1) -> None:
    global text
    count = text.count(old)
    if count != expected:
        raise SystemExit(
            f"expected exactly {expected} match(es), found {count}: {old[:120]!r}"
        )
    text = text.replace(old, new, expected)


replace_exact(
'''            ensure_owned_nap_registration(&control, &adapter, &attachment.interface, &owner)?;
            let clients = nap_client_interfaces(&attachment.interface)?;
            Ok(Box::new(BluezNapSubscription {
                connection: self.connection.clone(),
                control,
                adapter,
                bridge: attachment.interface,
                bluez_owner: owner,
                clients,
                pending: VecDeque::new(),
            }) as Box<dyn NapEventSubscription>)
''',
'''            ensure_owned_nap_registration(&control, &adapter, &attachment.interface, &owner)?;
            let clients = nap_client_interfaces(&attachment.interface)?;
            // Treat the initial authoritative bridge snapshot as attachments so a PANU
            // that connected between start_nap and subscription cannot be lost.
            let mut pending = VecDeque::new();
            queue_nap_client_changes(&BTreeSet::new(), &clients, &mut pending);
            Ok(Box::new(BluezNapSubscription {
                connection: self.connection.clone(),
                control,
                adapter,
                bridge: attachment.interface,
                bluez_owner: owner,
                clients,
                pending,
            }) as Box<dyn NapEventSubscription>)
''',
)

replace_exact(
'''    let proxy = match network_server_proxy(connection, adapter).await {
''',
'''    let proxy = match network_server_proxy(connection, adapter, &registration.bluez_owner).await {
''',
    expected=2,
)

replace_exact(
'''async fn network_server_proxy<'a>(
    connection: &'a Connection,
    adapter: &'a AdapterHandle,
) -> Result<Proxy<'a>, CoreError> {
    Proxy::new(
        connection,
        BLUEZ_SERVICE,
        adapter.as_str(),
        NETWORK_SERVER_INTERFACE,
    )
''',
'''async fn network_server_proxy<'a>(
    connection: &'a Connection,
    adapter: &'a AdapterHandle,
    bluez_owner: &'a str,
) -> Result<Proxy<'a>, CoreError> {
    // Target the exact BlueZ service instance that owns our lifecycle state. Using the
    // well-known org.bluez name here would allow a restart between ownership checks and
    // a destructive Unregister call to redirect that call to a different service instance.
    Proxy::new(
        connection,
        bluez_owner,
        adapter.as_str(),
        NETWORK_SERVER_INTERFACE,
    )
''',
)

replace_exact(
'''        "org.bluez.Error.NotAuthorized" | "org.freedesktop.DBus.Error.AccessDenied" => (
            ErrorKind::CapabilityUnavailable,
            "Bluetooth NAP registration is not authorized on this system",
        ),
        "org.freedesktop.DBus.Error.UnknownMethod"
''',
'''        "org.bluez.Error.NotAuthorized" | "org.freedesktop.DBus.Error.AccessDenied" => (
            ErrorKind::CapabilityUnavailable,
            "Bluetooth NAP registration is not authorized on this system",
        ),
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => (
            ErrorKind::BluezUnavailable,
            "BlueZ restarted or became unavailable during NAP registration",
        ),
        "org.freedesktop.DBus.Error.UnknownMethod"
''',
)

replace_exact(
'''                "org.bluez.Error.DoesNotExist"
                    | "org.freedesktop.DBus.Error.UnknownObject"
                    | "org.freedesktop.DBus.Error.UnknownInterface"
                    | "org.freedesktop.DBus.Error.UnknownMethod"
''',
'''                "org.bluez.Error.DoesNotExist"
                    | "org.freedesktop.DBus.Error.UnknownObject"
                    | "org.freedesktop.DBus.Error.UnknownInterface"
                    | "org.freedesktop.DBus.Error.UnknownMethod"
                    | "org.freedesktop.DBus.Error.ServiceUnknown"
                    | "org.freedesktop.DBus.Error.NameHasNoOwner"
''',
)

replace_exact(
'''    #[test]
    fn nap_client_changes_are_deterministic_and_backend_neutral() {
''',
'''    #[test]
    fn nap_initial_client_snapshot_reports_existing_clients() {
        let current = BTreeSet::from([bridge("bnep7"), bridge("bnep8")]);
        let mut pending = VecDeque::new();

        queue_nap_client_changes(&BTreeSet::new(), &current, &mut pending);
        assert_eq!(
            pending.pop_front(),
            Some(NapEvent::ClientAttached(PanAttachment {
                role: PanRole::Nap,
                interface: bridge("bnep7"),
                peer: None,
            }))
        );
        assert_eq!(
            pending.pop_front(),
            Some(NapEvent::ClientAttached(PanAttachment {
                role: PanRole::Nap,
                interface: bridge("bnep8"),
                peer: None,
            }))
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn nap_client_changes_are_deterministic_and_backend_neutral() {
''',
)

replace_exact(
'''        assert_eq!(
            nap_register_method_error("org.bluez.Error.NotSupported").0,
            ErrorKind::CapabilityUnavailable
        );
    }
''',
'''        assert_eq!(
            nap_register_method_error("org.bluez.Error.NotSupported").0,
            ErrorKind::CapabilityUnavailable
        );
        assert_eq!(
            nap_register_method_error("org.freedesktop.DBus.Error.ServiceUnknown").0,
            ErrorKind::BluezUnavailable
        );
    }
''',
)

path.write_text(text)
