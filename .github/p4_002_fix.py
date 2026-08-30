from pathlib import Path

path = Path("crates/blueroute-linux/src/bluez.rs")
text = path.read_text()
text = text.replace(
    "use zbus::zvariant::{OwnedInterfaceName, OwnedObjectPath, OwnedValue};",
    "use zbus::names::{BusName, OwnedInterfaceName};\nuse zbus::zvariant::{OwnedObjectPath, OwnedValue};",
)
old = '''    proxy
        .name_has_owner(BLUEZ_SERVICE.try_into().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "BlueZ service name is invalid",
                error.to_string(),
            )
        })?)
        .await
'''
new = '''    let service_name = BusName::try_from(BLUEZ_SERVICE).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::Internal,
            "BlueZ service name is invalid",
            error.to_string(),
        )
    })?;
    proxy
        .name_has_owner(service_name)
        .await
'''
if old not in text:
    raise SystemExit("expected BlueZ service-name block was not found")
text = text.replace(old, new)
path.write_text(text)
