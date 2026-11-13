//! This computer's display name (P12-009). A label people see, nothing more: it is never part of
//! the node's identity, keys, membership or authorization, and changing it changes none of them.
//! Stored owner-only in the state directory; absent means the default name.

use std::path::PathBuf;

use blueroute_core::{CoreError, DisplayName, ErrorKind, Secret};
use blueroute_linux::SecretFileStore;

const FILE_NAME: &str = "device-name-v1";
const HEADER: &str = "BLUEROUTE_DEVICE_NAME_V1";
const MAX_BYTES: usize = 512;

pub struct DeviceNameStore {
    secrets: SecretFileStore,
}

impl DeviceNameStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            secrets: SecretFileStore::new(directory),
        }
    }

    /// `None` when no name was ever set. A corrupt file is an error, never silently replaced.
    pub fn load(&self) -> Result<Option<DisplayName>, CoreError> {
        let Some(bytes) = self.secrets.load(FILE_NAME)? else {
            return Ok(None);
        };
        if bytes.expose().len() > MAX_BYTES {
            return Err(invalid("device name file exceeds size limit"));
        }
        let document = std::str::from_utf8(bytes.expose())
            .map_err(|_| invalid("device name file is not valid UTF-8"))?;
        let mut lines = document.lines();
        if lines.next() != Some(HEADER) {
            return Err(invalid("missing or unsupported device name format version"));
        }
        let name = lines
            .next()
            .ok_or_else(|| invalid("device name file has no name"))?;
        if lines.next().is_some() {
            return Err(invalid("device name file holds more than one name"));
        }
        DisplayName::new(name)
            .map(Some)
            .map_err(|_| invalid("stored device name is not a valid display name"))
    }

    pub fn save(&self, name: &DisplayName) -> Result<(), CoreError> {
        let text = format!("{HEADER}\n{}\n", name.as_str());
        self.secrets
            .save(FILE_NAME, &Secret::new(text.into_bytes()))
    }
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidState, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "blueroute-device-name-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn names_round_trip_and_unset_is_none() {
        let dir = directory();
        let store = DeviceNameStore::new(&dir);
        assert_eq!(store.load().unwrap(), None);
        let name = DisplayName::new("Kitchen laptop ☕").unwrap();
        store.save(&name).unwrap();
        assert_eq!(store.load().unwrap(), Some(name));
        let renamed = DisplayName::new("Workshop").unwrap();
        store.save(&renamed).unwrap();
        assert_eq!(store.load().unwrap(), Some(renamed));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_files_are_refused_not_replaced() {
        for body in [
            "WRONG_HEADER\nname\n",
            "BLUEROUTE_DEVICE_NAME_V1\n",
            "BLUEROUTE_DEVICE_NAME_V1\none\ntwo\n",
            "BLUEROUTE_DEVICE_NAME_V1\n\u{202e}evil\n",
        ] {
            let dir = directory();
            let store = DeviceNameStore::new(&dir);
            store
                .secrets
                .save(FILE_NAME, &Secret::new(body.as_bytes().to_vec()))
                .unwrap();
            assert_eq!(
                store.load().unwrap_err().kind(),
                ErrorKind::InvalidState,
                "{body:?}"
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
