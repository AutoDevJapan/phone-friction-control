use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

pub fn load<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => ron::from_str(&text)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn save<T: serde::Serialize>(path: &Path, state: &T) -> io::Result<()> {
    let text = ron::to_string(state).map_err(io::Error::other)?;
    replace_with(path, |file| file.write_all(text.as_bytes()))
}

fn replace_with(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing state directory"))?;
    fs::create_dir_all(parent)?;
    // One app instance writes on its UI thread. An interrupted staging file is never loaded.
    let staging = path.with_extension("pending");
    let mut file = File::create(&staging)?;
    write(&mut file)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&staging, path)?;
    #[cfg(target_os = "android")]
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
}
