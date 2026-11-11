use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct StateStore {
    path: PathBuf,
    ready: bool,
}

impl StateStore {
    #[cfg(any(target_os = "android", target_os = "ios", test))]
    pub fn new(path: PathBuf) -> Self {
        Self { path, ready: false }
    }

    #[cfg(any(target_os = "android", test))]
    pub fn load<T: serde::de::DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        self.ready = false;
        let result = load(&self.path);
        self.ready = result.is_ok();
        result
    }

    #[cfg(any(not(target_os = "android"), test))]
    pub fn load_and_migrate<T: serde::de::DeserializeOwned + serde::Serialize>(
        &mut self,
    ) -> io::Result<Option<T>> {
        self.load_and_migrate_with(|_| {})
    }

    pub(crate) fn load_and_migrate_with<T: serde::de::DeserializeOwned + serde::Serialize>(
        &mut self,
        update: impl FnOnce(&mut T),
    ) -> io::Result<Option<T>> {
        self.ready = false;
        let mut result = load(&self.path);
        if let Ok(Some(value)) = &mut result {
            update(value);
            let canonical = ron::to_string(value).map_err(io::Error::other)?;
            if fs::read_to_string(&self.path)? != canonical {
                save(&self.path, value)?;
            } else {
                remove_staging(&self.path)?;
            }
        } else if matches!(&result, Ok(None)) {
            remove_staging(&self.path)?;
        }
        self.ready = result.is_ok();
        result
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn save<T: serde::Serialize>(&self, state: &T) -> io::Result<()> {
        if !self.ready {
            return Err(io::Error::other("State has not been read successfully"));
        }
        save(&self.path, state)
    }
}

fn remove_staging(path: &Path) -> io::Result<()> {
    // Only the owning migration may discard an interrupted write; readers stay side-effect free.
    match fs::remove_file(path.with_extension("pending")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn load<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => ron::from_str(&text)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn save<T: serde::Serialize>(path: &Path, state: &T) -> io::Result<()> {
    let text = ron::to_string(state).map_err(io::Error::other)?;
    replace_with(path, |file| file.write_all(text.as_bytes()))
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) fn write_atomic<T: serde::Serialize>(path: &Path, state: &T) -> io::Result<()> {
    save(path, state)
}

fn replace_with(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing state directory"))?;
    fs::create_dir_all(parent)?;
    // Owning writers serialize each path; Android UI/guard transactions share a lock.
    // Never load staging files.
    let staging = path.with_extension("pending");
    let mut file = File::create(&staging)?;
    write(&mut file)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&staging, path)?;
    #[cfg(any(target_os = "android", target_os = "ios"))]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "phone-control-state-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("state.ron")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reader_does_not_serialize_or_overwrite_a_newer_owner_value() {
        #[derive(serde::Deserialize)]
        struct ReadOnly(u64);
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.0).unwrap();
        fs::write(fixture.path(), "(44)").unwrap();
        let mut reader = StateStore::new(fixture.path());
        assert_eq!(reader.load::<ReadOnly>().unwrap().unwrap().0, 44);
        fs::write(fixture.path(), "(99)").unwrap();
        assert_eq!(reader.load::<ReadOnly>().unwrap().unwrap().0, 99);
        assert_eq!(fs::read_to_string(fixture.path()).unwrap(), "(99)");
    }
    #[test]
    fn migration_serialization_failure_keeps_store_unready_and_file_unchanged() {
        #[derive(serde::Deserialize)]
        struct Fails(u64);
        impl serde::Serialize for Fails {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                let _ = self.0;
                Err(serde::ser::Error::custom("injected serialization failure"))
            }
        }
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.0).unwrap();
        fs::write(fixture.path(), "(44)").unwrap();
        let original = fs::read(fixture.path()).unwrap();
        let mut store = StateStore::new(fixture.path());
        assert!(store.load_and_migrate::<Fails>().is_err());
        assert!(!store.is_ready());
        assert!(store.save(&vec![0_u64]).is_err());
        assert_eq!(fs::read(fixture.path()).unwrap(), original);
    }
    #[test]
    fn loading_legacy_file_rewrites_without_a_metric_change() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Legacy {
            current: u64,
            #[serde(default, skip_serializing)]
            history: Vec<u64>,
            deadline: Option<i64>,
        }
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.0).unwrap();
        fs::write(
            fixture.path(),
            "(current:44,history:[12,13],deadline:Some(123456))",
        )
        .unwrap();
        let mut store = StateStore::new(fixture.path());
        let state = store.load_and_migrate::<Legacy>().unwrap().unwrap();
        assert_eq!(state.history, [12, 13]);
        assert_eq!(state.current, 44);
        assert_eq!(state.deadline, Some(123456));
        let disk = fs::read_to_string(fixture.path()).unwrap();
        assert!(!disk.contains("history"));
        let restored: Legacy = ron::from_str(&disk).unwrap();
        assert_eq!(restored.current, 44);
        assert_eq!(restored.deadline, state.deadline);
    }

    #[test]
    fn successful_owner_migration_discards_a_previous_interrupted_staging_record() {
        let fixture = Fixture::new();
        save(&fixture.path(), &vec![44_u64]).unwrap();
        fs::write(fixture.path().with_extension("pending"), "[99]").unwrap();
        let mut store = StateStore::new(fixture.path());
        assert_eq!(
            store.load_and_migrate::<Vec<u64>>().unwrap(),
            Some(vec![44])
        );
        assert!(!fixture.path().with_extension("pending").exists());
    }
    #[test]
    fn successful_save_replaces_existing_state() {
        let fixture = Fixture::new();
        assert_eq!(load::<Vec<u64>>(&fixture.path()).unwrap(), None);
        save(&fixture.path(), &vec![120, 30, 2_800_000_u64]).unwrap();
        save(&fixture.path(), &vec![60, 15, 3_000_000_u64]).unwrap();
        assert_eq!(
            load::<Vec<u64>>(&fixture.path()).unwrap(),
            Some(vec![60, 15, 3_000_000])
        );
    }

    #[test]
    fn interrupted_write_keeps_previous_valid_state() {
        let fixture = Fixture::new();
        save(&fixture.path(), &vec![120, 30, 2_800_000_u64]).unwrap();
        let original = fs::read(fixture.path()).unwrap();
        let result = replace_with(&fixture.path(), |file| {
            file.write_all(b"[60,")?;
            Err(io::Error::other("injected write interruption"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(fixture.path()).unwrap(), original);
        assert_eq!(
            load::<Vec<u64>>(&fixture.path()).unwrap(),
            Some(vec![120, 30, 2_800_000])
        );
        save(&fixture.path(), &vec![60_u64]).unwrap();
        assert_eq!(load::<Vec<u64>>(&fixture.path()).unwrap(), Some(vec![60]));
    }

    #[test]
    fn failed_load_prevents_overwrite_until_successful_reload() {
        let fixture = Fixture::new();
        save(&fixture.path(), &vec![120_u64, 30, 2_800_000]).unwrap();
        let valid = fs::read(fixture.path()).unwrap();
        fs::write(fixture.path(), b"[120,").unwrap();
        let unreadable = fs::read(fixture.path()).unwrap();
        let mut store = StateStore::new(fixture.path());
        assert!(store.save(&vec![0_u64]).is_err());
        assert!(store.load::<Vec<u64>>().is_err());
        assert!(!store.is_ready());
        assert!(store.save(&vec![0_u64]).is_err());
        assert_eq!(fs::read(fixture.path()).unwrap(), unreadable);
        fs::write(fixture.path(), valid).unwrap();
        assert_eq!(
            store.load::<Vec<u64>>().unwrap(),
            Some(vec![120, 30, 2_800_000])
        );
        store.save(&vec![60_u64]).unwrap();
        assert_eq!(load::<Vec<u64>>(&fixture.path()).unwrap(), Some(vec![60]));
    }

    #[test]
    fn missing_file_and_read_io_failure_are_distinct() {
        let fixture = Fixture::new();
        let mut store = StateStore::new(fixture.path());
        assert_eq!(store.load::<Vec<u64>>().unwrap(), None);
        store.save(&vec![120_u64]).unwrap();
        fs::remove_file(fixture.path()).unwrap();
        fs::create_dir(fixture.path()).unwrap();
        assert!(store.load::<Vec<u64>>().is_err());
        assert!(store.save(&vec![0_u64]).is_err());
        assert!(fixture.path().is_dir());
    }
}
