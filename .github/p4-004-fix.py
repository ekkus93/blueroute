from pathlib import Path

path = Path('crates/blueroute-linux/src/bluez.rs')
text = path.read_text()
text = text.replace(
    '''    proxy\n        .set_property(TRUSTED_PROPERTY, trusted)\n        .await\n        .map_err(trust_error)\n}\n\nfn trust_error(error: zbus::Error) -> CoreError {''',
    '''    proxy\n        .set_property(TRUSTED_PROPERTY, trusted)\n        .await\n        .map_err(trust_property_error)\n}\n\nfn trust_property_error(error: zbus::fdo::Error) -> CoreError {\n    let kind = match &error {\n        zbus::fdo::Error::UnknownObject(_) => ErrorKind::InvalidState,\n        zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_) => {\n            ErrorKind::AuthenticationFailed\n        }\n        _ => ErrorKind::AuthenticationFailed,\n    };\n    CoreError::with_diagnostic(\n        kind,\n        "failed to change Bluetooth trust state",\n        error.to_string(),\n    )\n}\n\nfn trust_error(error: zbus::Error) -> CoreError {''',
)
path.write_text(text)
