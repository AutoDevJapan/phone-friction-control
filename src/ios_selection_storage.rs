//! Unconnected product candidate: bytes must come from the public native codec.
//! No Swift layout, token construction, or native codec implementation here.

struct SelectionBytes {
    committed: Option<Vec<u8>>,
    load_failed: std::cell::Cell<bool>,
    verified: std::cell::Cell<bool>,
}

pub(crate) struct SelectionStore {
    disk: crate::persistence::StateStore,
    bytes: SelectionBytes,
}

/// Native implementations must return owned bytes and decoded owned values.
/// Neither operation may retain a borrowed Data/NSData buffer after returning.
pub(crate) trait SelectionCodec {
    type Selection: PartialEq;
    fn encode(&self, value: &Self::Selection) -> std::io::Result<Vec<u8>>;
    fn decode(&self, bytes: &[u8]) -> std::io::Result<Self::Selection>;
}

/// Binds restoration and publication to one codec instance. Opening an existing
/// file always validates it before the caller can publish a replacement.
pub(crate) struct RuntimeSelectionStore<C: SelectionCodec> {
    store: SelectionStore,
    codec: C,
    // The sidecar is deliberately retained on disk: removing a locked inode
    // would allow a new opener to lock a different file at the same pathname.
    _ownership: std::fs::File,
}

impl<C: SelectionCodec> RuntimeSelectionStore<C> {
    pub(crate) fn open(
        path: std::path::PathBuf,
        codec: C,
    ) -> std::io::Result<(Self, Option<C::Selection>)> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("missing parent"))?;
        std::fs::create_dir_all(parent)?;
        let name = path
            .file_name()
            .ok_or_else(|| std::io::Error::other("missing filename"))?;
        let mut lock_name = name.to_os_string();
        lock_name.push(".owner-lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ | FILE_SHARE_WRITE, deliberately no DELETE.
            // Keep the locked directory entry pinned throughout the session.
            options.share_mode(0x1 | 0x2);
        }
        let ownership = options.open(parent.join(lock_name))?;
        ownership.try_lock().map_err(std::io::Error::other)?;
        let store = SelectionStore::open(path)?;
        let restored = store.restore(|bytes| codec.decode(bytes))?;
        Ok((
            Self {
                store,
                codec,
                _ownership: ownership,
            },
            restored,
        ))
    }

    pub(crate) fn commit(
        &mut self,
        selection: &C::Selection,
    ) -> Result<(), PublishError<std::io::Error>> {
        let codec = &self.codec;
        self.store.save_checked(
            selection,
            |value| codec.encode(value),
            |bytes, original| Ok(codec.decode(bytes)? == *original),
        )
    }
}

impl SelectionStore {
    pub(crate) fn save_checked<T>(
        &mut self,
        selection: &T,
        encode: impl FnOnce(&T) -> std::io::Result<Vec<u8>>,
        decode_and_compare: impl FnOnce(&[u8], &T) -> std::io::Result<bool>,
    ) -> Result<(), PublishError<std::io::Error>> {
        self.save(selection, |value| {
            let bytes = encode(value)?;
            if bytes.is_empty() || !decode_and_compare(&bytes, value)? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Selection codec roundtrip mismatch",
                ));
            }
            Ok(bytes)
        })
    }
    pub(crate) fn open(path: std::path::PathBuf) -> std::io::Result<Self> {
        let mut disk = crate::persistence::StateStore::new(path);
        // Vec<u8> contains only bytes supplied by the public codec, never a
        // serialized Swift allocation or reconstructed opaque token.
        let saved = disk.load::<Vec<u8>>()?;
        Ok(Self {
            disk,
            bytes: SelectionBytes::loaded::<std::io::Error>(Ok(saved))?,
        })
    }

    pub(crate) fn restore<T>(
        &self,
        decode: impl FnOnce(&[u8]) -> std::io::Result<T>,
    ) -> std::io::Result<Option<T>> {
        self.bytes.restore(decode).map_err(|error| match error {
            RestoreError::Blocked => std::io::Error::other("Selection recovery required"),
            RestoreError::Decode(error) => error,
        })
    }

    fn save<T>(
        &mut self,
        selection: &T,
        encode: impl FnOnce(&T) -> std::io::Result<Vec<u8>>,
    ) -> Result<(), PublishError<std::io::Error>> {
        let disk = &self.disk;
        self.bytes
            .encode_and_publish(selection, encode, |bytes| disk.save(&bytes))
    }
}

impl SelectionBytes {
    fn encode_and_publish<T, E>(
        &mut self,
        selection: &T,
        encode: impl FnOnce(&T) -> Result<Vec<u8>, E>,
        atomic_write: impl FnOnce(&[u8]) -> Result<(), E>,
    ) -> Result<(), PublishError<E>> {
        if self.load_failed.get() || !self.verified.get() {
            return Err(PublishError::LoadFailed);
        }
        let bytes = encode(selection).map_err(PublishError::Encode)?;
        self.publish(bytes, atomic_write)
    }
    fn loaded<E>(read: Result<Option<Vec<u8>>, E>) -> Result<Self, E> {
        match read {
            Ok(committed) => Ok(Self {
                verified: std::cell::Cell::new(committed.is_none()),
                committed,
                load_failed: std::cell::Cell::new(false),
            }),
            Err(error) => Err(error),
        }
    }

    fn blocked_after_load_failure() -> Self {
        Self {
            committed: None,
            verified: std::cell::Cell::new(false),
            load_failed: std::cell::Cell::new(true),
        }
    }

    fn restore<T, E>(
        &self,
        decode: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<Option<T>, RestoreError<E>> {
        if self.load_failed.get() {
            return Err(RestoreError::Blocked);
        }
        let result = self.committed.as_deref().map(decode).transpose();
        if result.is_err() {
            self.load_failed.set(true);
        }
        if result.is_ok() {
            self.verified.set(true);
        }
        result.map_err(RestoreError::Decode)
    }

    fn publish<E>(
        &mut self,
        encoded: Vec<u8>,
        atomic_write: impl FnOnce(&[u8]) -> Result<(), E>,
    ) -> Result<(), PublishError<E>> {
        if self.load_failed.get() || !self.verified.get() {
            return Err(PublishError::LoadFailed);
        }
        if encoded.is_empty() {
            return Err(PublishError::EmptyEncoding);
        }
        if let Err(error) = atomic_write(&encoded) {
            self.load_failed.set(true);
            return Err(PublishError::Write(error));
        }
        self.committed = Some(encoded);
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum RestoreError<E> {
    Blocked,
    Decode(E),
}
#[derive(Debug, PartialEq)]
pub enum PublishError<E> {
    Encode(E),
    LoadFailed,
    EmptyEncoding,
    Write(E),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_adapter_restores_before_commit_and_rejects_invalid_disk() {
        struct Codec;
        impl SelectionCodec for Codec {
            type Selection = u8;
            fn encode(&self, value: &u8) -> std::io::Result<Vec<u8>> {
                Ok(vec![*value])
            }
            fn decode(&self, bytes: &[u8]) -> std::io::Result<u8> {
                match bytes {
                    [value] => Ok(*value),
                    _ => Err(std::io::Error::other("invalid codec bytes")),
                }
            }
        }
        let root = std::env::temp_dir().join(format!(
            "ios-runtime-codec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("selection.ron");
        let (mut session, initial) = RuntimeSelectionStore::open(path.clone(), Codec).unwrap();
        assert_eq!(initial, None);
        #[cfg(windows)]
        assert!(std::fs::remove_file(root.join("selection.ron.owner-lock")).is_err());
        assert!(RuntimeSelectionStore::open(path.clone(), Codec).is_err());
        assert!(RuntimeSelectionStore::open(root.join(".").join("selection.ron"), Codec).is_err());
        session.commit(&7).unwrap();
        drop(session);
        let (mut reopened, restored) = RuntimeSelectionStore::open(path.clone(), Codec).unwrap();
        assert_eq!(restored, Some(7));
        assert!(RuntimeSelectionStore::open(path.clone(), Codec).is_err());
        assert!(RuntimeSelectionStore::open(root.join(".").join("selection.ron"), Codec).is_err());
        reopened.commit(&8).unwrap();
        drop(reopened);
        std::fs::write(&path, "[1,2]").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(RuntimeSelectionStore::open(path.clone(), Codec).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(root.join("selection.ron.owner-lock")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn existing_file_requires_its_own_codec_validation_before_replacement() {
        let mut state = SelectionBytes::loaded::<()>(Ok(Some(vec![9, 8]))).unwrap();
        assert_eq!(
            state.encode_and_publish::<_, ()>(
                &7,
                |_| panic!("unverified encode"),
                |_| panic!("unverified write")
            ),
            Err(PublishError::LoadFailed)
        );
        state.restore::<_, ()>(|b| Ok(b.to_vec())).unwrap();
        state
            .encode_and_publish(&7u8, |v| Ok::<_, ()>(vec![*v]), |_| Ok(()))
            .unwrap();
        assert_eq!(state.restore::<_, ()>(|b| Ok(b[0])).unwrap(), Some(7));
    }
    #[test]
    fn checked_codec_rejects_mismatch_before_disk_publication() {
        let root = std::env::temp_dir().join(format!(
            "ios-roundtrip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("selection.ron");
        let mut store = SelectionStore::open(path.clone()).unwrap();
        store
            .save_checked(&7u8, |v| Ok(vec![*v]), |b, v| Ok(b == [*v]))
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(
            store
                .save_checked(&8u8, |_| Ok(vec![9]), |b, v| Ok(b == [*v]))
                .is_err()
        );
        assert!(
            store
                .save_checked(
                    &8u8,
                    |_| Ok(vec![8]),
                    |_, _| Err(std::io::Error::other("decode"))
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(store);
        let reopened = SelectionStore::open(path.clone()).unwrap();
        assert_eq!(reopened.restore(|b| Ok(b[0])).unwrap(), Some(7));
        drop(reopened);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn opening_invalid_codec_preserves_noncanonical_disk_bytes() {
        let root = std::env::temp_dir().join(format!(
            "ios-codec-readonly-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("selection.ron");
        let original = b"[ 9, 8, ]\n";
        std::fs::write(&path, original).unwrap();
        let mut store = SelectionStore::open(path.clone()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(
            store
                .restore::<()>(|_| Err(std::io::Error::other("invalid codec")))
                .is_err()
        );
        assert!(store.save(&7u8, |v| Ok(vec![*v])).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        drop(store);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn publish_then_error_and_decode_error_block_further_use() {
        let mut state = SelectionBytes::loaded::<()>(Ok(Some(vec![1, 2]))).unwrap();
        state.restore::<_, ()>(|b| Ok(b.to_vec())).unwrap();
        let mut disk = vec![1, 2];
        assert!(
            state
                .publish(vec![3], |bytes| {
                    disk = bytes.to_vec();
                    Err("sync after rename")
                })
                .is_err()
        );
        assert_eq!(disk, vec![3]);
        assert_eq!(
            state.restore::<(), ()>(|_| panic!("uncertain bytes")),
            Err(RestoreError::Blocked)
        );
        assert_eq!(
            state.publish::<()>(vec![4], |_| panic!("blocked write")),
            Err(PublishError::LoadFailed)
        );
        let reopened = SelectionBytes::loaded::<()>(Ok(Some(disk))).unwrap();
        assert_eq!(reopened.restore::<_, ()>(|b| Ok(b[0])).unwrap(), Some(3));
        let mut invalid = SelectionBytes::loaded::<()>(Ok(Some(vec![9]))).unwrap();
        assert_eq!(
            invalid.restore::<(), _>(|_| Err("decode")),
            Err(RestoreError::Decode("decode"))
        );
        assert_eq!(
            invalid.publish::<()>(vec![8], |_| panic!("decode blocked write")),
            Err(PublishError::LoadFailed)
        );
    }
    #[test]
    fn disk_restart_restores_codec_bytes_and_failed_encode_keeps_file() {
        let root = std::env::temp_dir().join(format!(
            "ios-selection-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("selection.ron");
        let mut store = SelectionStore::open(path.clone()).unwrap();
        assert_eq!(store.restore(|_| Ok(0u8)).unwrap(), None);
        store.save(&7u8, |v| Ok(vec![*v])).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(
            store
                .save(&8u8, |_| Err(std::io::Error::other("codec failure")))
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(store);
        let restored = SelectionStore::open(path.clone()).unwrap();
        assert_eq!(restored.restore(|b| Ok(b[0])).unwrap(), Some(7));
        assert!(
            restored
                .restore::<()>(|_| Err(std::io::Error::other("decode failure")))
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(restored);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
    #[test]
    fn codec_consumer_only_publishes_successful_encoding() {
        let mut state = SelectionBytes::loaded::<()>(Ok(Some(vec![9]))).unwrap();
        state.restore::<_, ()>(|b| Ok(b.to_vec())).unwrap();
        assert_eq!(
            state.encode_and_publish(&(), |_| Err("codec"), |_| panic!("must not write")),
            Err(PublishError::Encode("codec"))
        );
        assert_eq!(
            state.restore::<_, ()>(|b| Ok(b.to_vec())).unwrap(),
            Some(vec![9])
        );
        state
            .encode_and_publish(
                &7u8,
                |v| Ok::<_, &str>(vec![*v]),
                |b| {
                    assert_eq!(b, &[7]);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(state.restore::<_, ()>(|b| Ok(b[0])).unwrap(), Some(7));
    }
    // Synthetic codec bytes only; these do not represent real Screen Time tokens.
    #[test]
    fn failed_publication_keeps_previous_restorable_bytes() {
        let mut state = SelectionBytes::loaded::<()>(Ok(Some(vec![1, 2]))).unwrap();
        state.restore::<_, ()>(|b| Ok(b.to_vec())).unwrap();
        assert_eq!(
            state.publish(vec![3], |_| Err("disk")),
            Err(PublishError::Write("disk"))
        );
        assert_eq!(
            state.restore::<_, ()>(|bytes| Ok(bytes.to_vec())),
            Err(RestoreError::Blocked)
        );
    }
    #[test]
    fn decoder_failure_does_not_replace_saved_selection() {
        let state = SelectionBytes::loaded::<()>(Ok(Some(vec![1, 2]))).unwrap();
        assert_eq!(
            state.restore::<(), _>(|_| Err("invalid codec bytes")),
            Err(RestoreError::Decode("invalid codec bytes"))
        );
        assert_eq!(
            state.restore::<_, ()>(|bytes| Ok(bytes.len())),
            Err(RestoreError::Blocked)
        );
    }
    #[test]
    fn failed_load_and_empty_encoding_never_write() {
        let mut blocked = SelectionBytes::blocked_after_load_failure();
        assert_eq!(
            blocked.publish::<()>(vec![3], |_| panic!("must not write")),
            Err(PublishError::LoadFailed)
        );
        let mut missing = SelectionBytes::loaded::<()>(Ok(None)).unwrap();
        assert_eq!(
            missing.restore::<(), ()>(|_| panic!("no saved value")),
            Ok(None)
        );
        assert_eq!(
            missing.publish::<()>(vec![], |_| panic!("must not write")),
            Err(PublishError::EmptyEncoding)
        );
        missing
            .publish(vec![4], |b| {
                assert_eq!(b, &[4]);
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(missing.restore::<_, ()>(|b| Ok(b[0])).unwrap(), Some(4));
    }
}
