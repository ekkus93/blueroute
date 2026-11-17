//! A test-only NetworkManager on a private bus, implementing exactly the D-Bus surface that
//! `NetworkManagerBackend` uses (TEST_COVERAGE_SPEC COV-3) and nothing else:
//!
//! - `org.freedesktop.NetworkManager`: `Version`, `GetDevices`, `ActivateConnection`,
//!   `DeactivateConnection`;
//! - `…Settings`: `ListConnections`, `AddConnection2`;
//! - `…Settings.Connection`: `GetSettings`, `Update2`, `Delete`;
//! - `…Device`: `Interface`, `DeviceType`, `Managed`, `State`, `ActiveConnection`, `Reapply`;
//! - `…Connection.Active`: `Connection`.
//!
//! State changes apply immediately, so the backend's polling waits succeed on their first check.
//! Every mutating call is logged with the profile it targets, including calls that fail, and any
//! method can be scripted to fail with `org.freedesktop.DBus.Error.Failed`.
//!
//! Profiles, devices and active connections live in fixed pools of object paths registered at
//! start; a path whose slot is empty answers `UnknownObject`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use zbus::connection::Builder;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, fdo, interface};

pub type Settings = HashMap<String, HashMap<String, OwnedValue>>;

pub const SERVICE: &str = "org.freedesktop.NetworkManager";
pub const VERSION: &str = "1.52.0-blueroute-fake";
const MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const PROFILE_SLOTS: u32 = 32;
const DEVICE_SLOTS: u32 = 16;
const ACTIVE_SLOTS: u32 = 64;
const DEVICE_TYPE_BRIDGE: u32 = 13;
const DEVICE_STATE_DISCONNECTED: u32 = 30;
const DEVICE_STATE_ACTIVATED: u32 = 100;

pub fn profile_path(id: u32) -> String {
    format!("{SETTINGS_PATH}/{id}")
}

fn device_path(id: u32) -> String {
    format!("{MANAGER_PATH}/Devices/{id}")
}

fn active_path(id: u32) -> String {
    format!("{MANAGER_PATH}/ActiveConnection/{id}")
}

fn path(value: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(value.to_owned()).expect("valid fake object path")
}

/// The numeric slot at the end of `value` if it lies under `prefix`.
fn slot(value: &str, prefix: &str) -> Option<u32> {
    value.strip_prefix(prefix)?.strip_prefix('/')?.parse().ok()
}

pub fn clone_settings(settings: &Settings) -> Settings {
    settings
        .iter()
        .map(|(group, values)| {
            let values = values
                .iter()
                .map(|(key, value)| (key.clone(), value.try_clone().expect("cloneable value")))
                .collect();
            (group.clone(), values)
        })
        .collect()
}

fn setting<'a>(settings: &'a Settings, group: &str, key: &str) -> Option<&'a str> {
    settings
        .get(group)?
        .get(key)
        .and_then(|value| <&str>::try_from(value).ok())
}

/// One mutating call as the fake received it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Call {
    pub method: &'static str,
    /// The settings profile the call targets (for `AddConnection2`, the path it created, or
    /// `"(none)"` if it failed; for `DeactivateConnection` and `Reapply`, the profile behind the
    /// active connection, or `"/"` if there is none).
    pub profile: String,
    pub device: Option<String>,
    pub flags: Option<u32>,
    pub failed: bool,
}

#[derive(Debug)]
struct Device {
    interface: String,
    device_type: u32,
    managed: bool,
    state: u32,
    active: Option<u32>,
}

#[derive(Default)]
struct State {
    profiles: BTreeMap<u32, Settings>,
    devices: BTreeMap<u32, Device>,
    actives: BTreeMap<u32, u32>,
    next_profile: u32,
    next_device: u32,
    next_active: u32,
    calls: Vec<Call>,
    failing: BTreeSet<&'static str>,
}

impl State {
    fn scripted(&self, method: &'static str) -> fdo::Result<()> {
        if self.failing.contains(method) {
            Err(fdo::Error::Failed(format!("scripted {method} failure")))
        } else {
            Ok(())
        }
    }

    fn log(&mut self, method: &'static str, profile: String, device: Option<String>) -> bool {
        let failed = self.failing.contains(method);
        self.calls.push(Call {
            method,
            profile,
            device,
            flags: None,
            failed,
        });
        failed
    }

    fn insert_profile(&mut self, settings: Settings) -> fdo::Result<u32> {
        self.next_profile += 1;
        let id = self.next_profile;
        if id > PROFILE_SLOTS {
            return Err(fdo::Error::LimitsExceeded(
                "fake profile pool exhausted".into(),
            ));
        }
        self.profiles.insert(id, settings);
        Ok(id)
    }

    fn insert_device(&mut self, interface: &str, device_type: u32) -> u32 {
        self.next_device += 1;
        let id = self.next_device;
        assert!(id <= DEVICE_SLOTS, "fake device pool exhausted");
        self.devices.insert(
            id,
            Device {
                interface: interface.to_owned(),
                device_type,
                managed: true,
                state: DEVICE_STATE_DISCONNECTED,
                active: None,
            },
        );
        id
    }

    fn activate(&mut self, profile: u32, device: u32) -> fdo::Result<u32> {
        self.next_active += 1;
        let active = self.next_active;
        if active > ACTIVE_SLOTS {
            return Err(fdo::Error::LimitsExceeded(
                "fake active pool exhausted".into(),
            ));
        }
        let device = self.devices.get_mut(&device).expect("device exists");
        if let Some(previous) = device.active.replace(active) {
            self.actives.remove(&previous);
        }
        device.state = DEVICE_STATE_ACTIVATED;
        self.actives.insert(active, profile);
        Ok(active)
    }

    fn deactivate(&mut self, active: u32) {
        self.actives.remove(&active);
        for device in self.devices.values_mut() {
            if device.active == Some(active) {
                device.active = None;
                device.state = DEVICE_STATE_DISCONNECTED;
            }
        }
    }

    fn profile_of_active(&self, active: Option<u32>) -> String {
        active
            .and_then(|active| self.actives.get(&active))
            .map_or_else(|| "/".to_owned(), |profile| profile_path(*profile))
    }
}

type Shared = Arc<Mutex<State>>;

fn lock(state: &Shared) -> MutexGuard<'_, State> {
    state.lock().expect("fake NetworkManager state")
}

struct Manager {
    state: Shared,
}

#[interface(name = "org.freedesktop.NetworkManager")]
impl Manager {
    fn get_devices(&self) -> fdo::Result<Vec<OwnedObjectPath>> {
        let state = lock(&self.state);
        state.scripted("GetDevices")?;
        Ok(state
            .devices
            .keys()
            .map(|id| path(&device_path(*id)))
            .collect())
    }

    fn activate_connection(
        &self,
        connection: OwnedObjectPath,
        device: OwnedObjectPath,
        _specific_object: OwnedObjectPath,
    ) -> fdo::Result<OwnedObjectPath> {
        let mut state = lock(&self.state);
        let device_text = device.as_str().to_owned();
        if state.log(
            "ActivateConnection",
            connection.as_str().to_owned(),
            Some(device_text.clone()),
        ) {
            state.scripted("ActivateConnection")?;
        }
        let profile = slot(connection.as_str(), SETTINGS_PATH)
            .filter(|id| state.profiles.contains_key(id))
            .ok_or_else(|| fdo::Error::UnknownObject(connection.as_str().to_owned()))?;
        let device = if device_text == "/" {
            let settings = &state.profiles[&profile];
            if setting(settings, "connection", "type") != Some("bridge") {
                return Err(fdo::Error::Failed(
                    "only a bridge profile can be activated without a device".into(),
                ));
            }
            let interface = setting(settings, "connection", "interface-name")
                .ok_or_else(|| fdo::Error::Failed("bridge profile has no interface".into()))?
                .to_owned();
            match state
                .devices
                .iter()
                .find(|(_, value)| value.interface == interface)
            {
                Some((id, _)) => *id,
                None => state.insert_device(&interface, DEVICE_TYPE_BRIDGE),
            }
        } else {
            slot(&device_text, &format!("{MANAGER_PATH}/Devices"))
                .filter(|id| state.devices.contains_key(id))
                .ok_or_else(|| fdo::Error::UnknownObject(device_text.clone()))?
        };
        let active = state.activate(profile, device)?;
        Ok(path(&active_path(active)))
    }

    fn deactivate_connection(&self, active_connection: OwnedObjectPath) -> fdo::Result<()> {
        let mut state = lock(&self.state);
        let active = slot(
            active_connection.as_str(),
            &format!("{MANAGER_PATH}/ActiveConnection"),
        );
        let profile = state.profile_of_active(active);
        if state.log("DeactivateConnection", profile, None) {
            state.scripted("DeactivateConnection")?;
        }
        let active = active
            .filter(|id| state.actives.contains_key(id))
            .ok_or_else(|| fdo::Error::UnknownObject(active_connection.as_str().to_owned()))?;
        state.deactivate(active);
        Ok(())
    }

    #[zbus(property)]
    fn version(&self) -> String {
        VERSION.to_owned()
    }
}

struct SettingsManager {
    state: Shared,
}

#[interface(name = "org.freedesktop.NetworkManager.Settings")]
impl SettingsManager {
    fn list_connections(&self) -> fdo::Result<Vec<OwnedObjectPath>> {
        let state = lock(&self.state);
        state.scripted("ListConnections")?;
        Ok(state
            .profiles
            .keys()
            .map(|id| path(&profile_path(*id)))
            .collect())
    }

    #[zbus(name = "AddConnection2")]
    fn add_connection2(
        &self,
        settings: Settings,
        flags: u32,
        _args: HashMap<String, OwnedValue>,
    ) -> fdo::Result<(OwnedObjectPath, HashMap<String, OwnedValue>)> {
        let mut state = lock(&self.state);
        let failed = state.failing.contains("AddConnection2");
        let created = if failed {
            None
        } else {
            Some(state.insert_profile(settings)?)
        };
        state.calls.push(Call {
            method: "AddConnection2",
            profile: created.map_or_else(|| "(none)".to_owned(), profile_path),
            device: None,
            flags: Some(flags),
            failed,
        });
        state.scripted("AddConnection2")?;
        let id = created.expect("created when not failing");
        Ok((path(&profile_path(id)), HashMap::new()))
    }
}

struct Profile {
    id: u32,
    state: Shared,
}

#[interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl Profile {
    fn get_settings(&self) -> fdo::Result<Settings> {
        let state = lock(&self.state);
        state.scripted("GetSettings")?;
        state
            .profiles
            .get(&self.id)
            .map(clone_settings)
            .ok_or_else(|| fdo::Error::UnknownObject(profile_path(self.id)))
    }

    #[zbus(name = "Update2")]
    fn update2(
        &self,
        settings: Settings,
        flags: u32,
        _args: HashMap<String, OwnedValue>,
    ) -> fdo::Result<HashMap<String, OwnedValue>> {
        let mut state = lock(&self.state);
        let failed = state.failing.contains("Update2");
        state.calls.push(Call {
            method: "Update2",
            profile: profile_path(self.id),
            device: None,
            flags: Some(flags),
            failed,
        });
        state.scripted("Update2")?;
        let stored = state
            .profiles
            .get_mut(&self.id)
            .ok_or_else(|| fdo::Error::UnknownObject(profile_path(self.id)))?;
        *stored = settings;
        Ok(HashMap::new())
    }

    fn delete(&self) -> fdo::Result<()> {
        let mut state = lock(&self.state);
        if state.log("Delete", profile_path(self.id), None) {
            state.scripted("Delete")?;
        }
        if state.profiles.remove(&self.id).is_none() {
            return Err(fdo::Error::UnknownObject(profile_path(self.id)));
        }
        let actives: Vec<u32> = state
            .actives
            .iter()
            .filter(|(_, profile)| **profile == self.id)
            .map(|(active, _)| *active)
            .collect();
        for active in actives {
            state.deactivate(active);
        }
        Ok(())
    }
}

struct DeviceObject {
    id: u32,
    state: Shared,
}

impl DeviceObject {
    fn read<T>(&self, read: impl FnOnce(&Device) -> T) -> fdo::Result<T> {
        lock(&self.state)
            .devices
            .get(&self.id)
            .map(read)
            .ok_or_else(|| fdo::Error::UnknownObject(device_path(self.id)))
    }
}

#[interface(name = "org.freedesktop.NetworkManager.Device")]
impl DeviceObject {
    fn reapply(&self, _settings: Settings, _version_id: u64, _flags: u32) -> fdo::Result<()> {
        let mut state = lock(&self.state);
        let active = state.devices.get(&self.id).and_then(|device| device.active);
        let profile = state.profile_of_active(active);
        if state.log("Reapply", profile, Some(device_path(self.id))) {
            state.scripted("Reapply")?;
        }
        if active.is_none() {
            return Err(fdo::Error::Failed("device is not active".into()));
        }
        Ok(())
    }

    #[zbus(property)]
    fn interface(&self) -> fdo::Result<String> {
        self.read(|device| device.interface.clone())
    }

    #[zbus(property)]
    fn managed(&self) -> fdo::Result<bool> {
        self.read(|device| device.managed)
    }

    #[zbus(property)]
    fn device_type(&self) -> fdo::Result<u32> {
        self.read(|device| device.device_type)
    }

    #[zbus(property)]
    fn state(&self) -> fdo::Result<u32> {
        self.read(|device| device.state)
    }

    #[zbus(property)]
    fn active_connection(&self) -> fdo::Result<OwnedObjectPath> {
        self.read(|device| path(&device.active.map_or_else(|| "/".to_owned(), active_path)))
    }
}

struct ActiveConnection {
    id: u32,
    state: Shared,
}

#[interface(name = "org.freedesktop.NetworkManager.Connection.Active")]
impl ActiveConnection {
    #[zbus(property)]
    fn connection(&self) -> fdo::Result<OwnedObjectPath> {
        lock(&self.state)
            .actives
            .get(&self.id)
            .map(|profile| path(&profile_path(*profile)))
            .ok_or_else(|| fdo::Error::UnknownObject(active_path(self.id)))
    }
}

/// The running fake. Dropping it without `stop` also releases the name when the connection
/// closes, so a failed scenario does not block the next one.
pub struct FakeNetworkManager {
    connection: Connection,
    state: Shared,
}

impl FakeNetworkManager {
    pub async fn start() -> zbus::Result<Self> {
        let state: Shared = Arc::default();
        let mut builder = Builder::session()?
            .name(SERVICE)?
            .serve_at(
                MANAGER_PATH,
                Manager {
                    state: state.clone(),
                },
            )?
            .serve_at(
                SETTINGS_PATH,
                SettingsManager {
                    state: state.clone(),
                },
            )?;
        for id in 1..=PROFILE_SLOTS {
            let state = state.clone();
            builder = builder.serve_at(profile_path(id), Profile { id, state })?;
        }
        for id in 1..=DEVICE_SLOTS {
            let state = state.clone();
            builder = builder.serve_at(device_path(id), DeviceObject { id, state })?;
        }
        for id in 1..=ACTIVE_SLOTS {
            let state = state.clone();
            builder = builder.serve_at(active_path(id), ActiveConnection { id, state })?;
        }
        let connection = builder.build().await?;
        Ok(Self { connection, state })
    }

    pub async fn stop(self) -> zbus::Result<()> {
        self.connection.release_name(SERVICE).await?;
        self.connection.close().await
    }

    pub fn unique_name(&self) -> String {
        self.connection
            .unique_name()
            .expect("connected fake has a unique name")
            .to_string()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Seed a profile and return its path.
    pub fn add_profile(&self, settings: Settings) -> String {
        profile_path(self.state().insert_profile(settings).expect("profile slot"))
    }

    /// Seed a device, optionally already active with `profile`, and return its path.
    pub fn add_device(&self, interface: &str, device_type: u32, profile: Option<&str>) -> String {
        let mut state = self.state();
        let id = state.insert_device(interface, device_type);
        if let Some(profile) = profile {
            let profile = slot(profile, SETTINGS_PATH).expect("seeded profile path");
            state.activate(profile, id).expect("active slot");
        }
        device_path(id)
    }

    pub fn settings(&self, profile: &str) -> Option<Settings> {
        let state = self.state();
        slot(profile, SETTINGS_PATH)
            .and_then(|id| state.profiles.get(&id))
            .map(clone_settings)
    }

    pub fn profiles(&self) -> Vec<String> {
        self.state()
            .profiles
            .keys()
            .map(|id| profile_path(*id))
            .collect()
    }

    /// The profile active on the device named `interface`, if any.
    pub fn active_profile(&self, interface: &str) -> Option<String> {
        let state = self.state();
        let device = state
            .devices
            .values()
            .find(|device| device.interface == interface)?;
        device
            .active
            .and_then(|active| state.actives.get(&active))
            .map(|profile| profile_path(*profile))
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state().calls.clone()
    }

    pub fn clear_calls(&self) {
        self.state().calls.clear();
    }

    pub fn fail(&self, method: &'static str) {
        self.state().failing.insert(method);
    }
}
