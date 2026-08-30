from pathlib import Path

path = Path('crates/blueroute-linux/src/bluez.rs')
text = path.read_text()
text = text.replace(
    '.map_err(|error| incoming_pairing_property_error("read Bluetooth Pairable state", error))?;',
    '.map_err(|error| incoming_pairing_error("read Bluetooth Pairable state", error))?;',
)
text = text.replace(
    'incoming_pairing_property_error("read Bluetooth Discoverable state", error)',
    'incoming_pairing_error("read Bluetooth Discoverable state", error)',
)
path.write_text(text)
