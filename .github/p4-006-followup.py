from pathlib import Path

path = Path("crates/blueroute-linux/src/pan.rs")
text = path.read_text()


def replace_once(old: str, new: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"expected exactly one match, found {count}: {old[:120]!r}")
    text = text.replace(old, new, 1)


replace_once(
'''            ensure_owned_nap_registration(&control, &adapter, &attachment.interface, &owner)?;
            let clients = nap_client_interfaces(&attachment.interface)?;
            Ok(Box::new(BluezNapSubscription {
                bridge: attachment.interface,
                clients,
                pending: VecDeque::new(),
            }) as Box<dyn NapEventSubscription>)
''',
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
)

replace_once(
'''struct BluezNapSubscription {
    bridge: NetworkInterfaceHandle,
    clients: BTreeSet<NetworkInterfaceHandle>,
    pending: VecDeque<NapEvent>,
}
''',
'''struct BluezNapSubscription {
    connection: Connection,
    control: Arc<NapControl>,
    adapter: AdapterHandle,
    bridge: NetworkInterfaceHandle,
    bluez_owner: String,
    clients: BTreeSet<NetworkInterfaceHandle>,
    pending: VecDeque<NapEvent>,
}
''',
)

replace_once(
'''        Box::pin(async move {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            loop {
                Timer::after(NAP_EVENT_POLL_INTERVAL).await;
                let current = nap_client_interfaces(&self.bridge)?;
''',
'''        Box::pin(async move {
            if !self.registration_is_current().await? {
                self.pending.clear();
                return Ok(None);
            }
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            loop {
                Timer::after(NAP_EVENT_POLL_INTERVAL).await;
                if !self.registration_is_current().await? {
                    self.pending.clear();
                    return Ok(None);
                }
                let current = nap_client_interfaces(&self.bridge)?;
''',
)

replace_once(
'''    }
}

async fn start_nap(
''',
'''    }
}

impl BluezNapSubscription {
    async fn registration_is_current(&self) -> Result<bool, CoreError> {
        let owner = current_bluez_owner(&self.connection).await?;
        if owner.as_deref() != Some(self.bluez_owner.as_str()) {
            return Ok(false);
        }
        nap_registration_is_active(
            &self.control,
            &self.adapter,
            &self.bridge,
            &self.bluez_owner,
        )
    }
}

async fn start_nap(
''',
)

replace_once(
'''    let proxy = network_server_proxy(connection, adapter).await?;
    match proxy
''',
'''    let proxy = match network_server_proxy(connection, adapter).await {
        Ok(proxy) => proxy,
        Err(error) => {
            return Err(cleanup_nap_setup_failure(control, adapter, error));
        }
    };
    match proxy
''',
)

replace_once(
'''fn clear_nap_registration(control: &NapControl, adapter: &AdapterHandle) -> Result<(), CoreError> {
    let mut states = control.states.lock().map_err(nap_state_lock_error)?;
    states.remove(adapter);
    Ok(())
}

fn ensure_owned_nap_registration(
''',
'''fn clear_nap_registration(control: &NapControl, adapter: &AdapterHandle) -> Result<(), CoreError> {
    let mut states = control.states.lock().map_err(nap_state_lock_error)?;
    states.remove(adapter);
    Ok(())
}

fn cleanup_nap_setup_failure(
    control: &NapControl,
    adapter: &AdapterHandle,
    setup_error: CoreError,
) -> CoreError {
    match clear_nap_registration(control, adapter) {
        Ok(()) => setup_error,
        Err(state_error) => {
            let setup_diagnostic = setup_error
                .diagnostic()
                .unwrap_or_else(|| setup_error.message());
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "Bluetooth NAP setup failed and local ownership state could not be cleared",
                format!("setup error: {setup_diagnostic}; state error: {state_error}"),
            )
        }
    }
}

fn nap_registration_is_active(
    control: &NapControl,
    adapter: &AdapterHandle,
    bridge: &NetworkInterfaceHandle,
    owner: &str,
) -> Result<bool, CoreError> {
    let states = control.states.lock().map_err(nap_state_lock_error)?;
    Ok(matches!(
        states.get(adapter),
        Some(NapLifecycleState::Active(registration))
            if registration.bluez_owner == owner && registration.bridge == *bridge
    ))
}

fn ensure_owned_nap_registration(
''',
)

replace_once(
'''    let states = control.states.lock().map_err(nap_state_lock_error)?;
    match states.get(adapter) {
        Some(NapLifecycleState::Active(registration))
            if registration.bluez_owner == owner && registration.bridge == *bridge =>
        {
            Ok(())
        }
        _ => Err(CoreError::new(
            ErrorKind::InvalidState,
            "NAP event subscription requires an active NAP owned by this backend",
        )),
    }
}
''',
'''    if nap_registration_is_active(control, adapter, bridge, owner)? {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::InvalidState,
            "NAP event subscription requires an active NAP owned by this backend",
        ))
    }
}
''',
)

replace_once(
'''    #[test]
    fn nap_registration_is_idempotent_for_same_owned_bridge() {
''',
'''    #[test]
    fn failed_nap_setup_cleanup_allows_retry() {
        let control = NapControl::default();
        let adapter = adapter();
        let registration = registration("br-blue", ":1.42");

        begin_nap_registration(&control, &adapter, &registration).unwrap();
        let error = cleanup_nap_setup_failure(
            &control,
            &adapter,
            CoreError::new(ErrorKind::BluezUnavailable, "proxy setup failed"),
        );
        assert_eq!(error.kind(), ErrorKind::BluezUnavailable);
        assert!(!control.states.lock().unwrap().contains_key(&adapter));
        assert_eq!(
            begin_nap_registration(&control, &adapter, &registration).unwrap(),
            None
        );
    }

    #[test]
    fn nap_registration_activity_requires_matching_active_state() {
        let control = NapControl::default();
        let adapter = adapter();
        let registration = registration("br-blue", ":1.42");

        begin_nap_registration(&control, &adapter, &registration).unwrap();
        assert!(!nap_registration_is_active(
            &control,
            &adapter,
            &registration.bridge,
            ":1.42"
        )
        .unwrap());
        activate_nap_registration(&control, &adapter, &registration).unwrap();
        assert!(nap_registration_is_active(
            &control,
            &adapter,
            &registration.bridge,
            ":1.42"
        )
        .unwrap());
        assert!(!nap_registration_is_active(
            &control,
            &adapter,
            &registration.bridge,
            ":1.99"
        )
        .unwrap());
        assert!(!nap_registration_is_active(
            &control,
            &adapter,
            &bridge("br-other"),
            ":1.42"
        )
        .unwrap());
        begin_nap_stop(&control, &adapter, Some(":1.42")).unwrap();
        assert!(!nap_registration_is_active(
            &control,
            &adapter,
            &registration.bridge,
            ":1.42"
        )
        .unwrap());
    }

    #[test]
    fn nap_registration_is_idempotent_for_same_owned_bridge() {
''',
)

path.write_text(text)
