from pathlib import Path

path = Path("crates/blueroute-linux/src/bluez.rs")
text = path.read_text()

old_const = 'const BLUEZ_ROOT_PATH: &str = "/";\n'
new_const = old_const + 'const BLUEZ_AGENT_MANAGER_PATH: &str = "/org/bluez";\n'
if text.count(old_const) != 1:
    raise SystemExit(f"expected exactly one BLUEZ_ROOT_PATH declaration, found {text.count(old_const)}")
text = text.replace(old_const, new_const, 1)

old_proxy = '''        BLUEZ_SERVICE,\n        BLUEZ_ROOT_PATH,\n        AGENT_MANAGER_INTERFACE,\n'''
new_proxy = '''        BLUEZ_SERVICE,\n        BLUEZ_AGENT_MANAGER_PATH,\n        AGENT_MANAGER_INTERFACE,\n'''
count = text.count(old_proxy)
if count != 3:
    raise SystemExit(f"expected exactly three AgentManager proxy constructions, found {count}")
text = text.replace(old_proxy, new_proxy)

path.write_text(text)

doc = Path("docs/P4-004-PAIRING.md")
doc_text = doc.read_text()
anchor = "BlueRoute performs pairing directly through `org.bluez.Device1.Pair` and changes BlueZ trust through the `org.bluez.Device1.Trusted` property. Production code does not invoke or parse `bluetoothctl`.\n"
addition = anchor + "\nBlueZ pairing-agent management is performed through `org.bluez.AgentManager1` at `/org/bluez`; `/` remains reserved for BlueZ's `org.freedesktop.DBus.ObjectManager`.\n"
if doc_text.count(anchor) != 1:
    raise SystemExit("pairing documentation anchor not found exactly once")
doc.write_text(doc_text.replace(anchor, addition, 1))
