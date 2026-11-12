//! Unconnected product candidate: bytes must come from the public native codec.
//! No Swift layout, token construction, or native codec implementation here.

pub struct SelectionBytes {
    committed: Option<Vec<u8>>,
    load_failed: bool,
}

pub(crate) struct SelectionStore {
    disk: crate::persistence::StateStore,
    bytes: SelectionBytes,
}

impl SelectionStore {
    pub(crate) fn open(path: std::path::PathBuf) -> std::io::Result<Self> {
        let mut disk = crate::persistence::StateStore::new(path);
        // Vec<u8> contains only bytes supplied by the public codec, never a
        // serialized Swift allocation or reconstructed opaque token.
        let saved = disk.load_and_migrate::<Vec<u8>>()?;
        Ok(Self {
            disk,
            bytes: SelectionBytes::loaded::<std::io::Error>(Ok(saved))?,
        })
    }

    pub(crate) fn restore<T>(
        &self,
        decode: impl FnOnce(&[u8]) -> std::io::Result<T>,
    ) -> std::io::Result<Option<T>> {
        self.bytes.restore(decode)
    }

    pub(crate) fn save<T>(
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
    pub fn encode_and_publish<T, E>(
        &mut self,
        selection: &T,
        encode: impl FnOnce(&T) -> Result<Vec<u8>, E>,
        atomic_write: impl FnOnce(&[u8]) -> Result<(), E>,
    ) -> Result<(), PublishError<E>> {
        if self.load_failed {
            return Err(PublishError::LoadFailed);
        }
        let bytes = encode(selection).map_err(PublishError::Encode)?;
        self.publish(bytes, atomic_write)
    }
    pub fn loaded<E>(read: Result<Option<Vec<u8>>, E>) -> Result<Self, E> {
        match read {
            Ok(committed) => Ok(Self {
                committed,
                load_failed: false,
            }),
            Err(error) => Err(error),
        }
    }

    pub fn blocked_after_load_failure() -> Self {
        Self {
            committed: None,
            load_failed: true,
        }
    }

    pub fn restore<T, E>(
        &self,
        decode: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        self.committed.as_deref().map(decode).transpose()
    }

    pub fn publish<E>(
        &mut self,
        encoded: Vec<u8>,
        atomic_write: impl FnOnce(&[u8]) -> Result<(), E>,
    ) -> Result<(), PublishError<E>> {
        if self.load_failed {
            return Err(PublishError::LoadFailed);
        }
        if encoded.is_empty() {
            return Err(PublishError::EmptyEncoding);
        }
        atomic_write(&encoded).map_err(PublishError::Write)?;
        self.committed = Some(encoded);
        Ok(())
    }
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
        assert_eq!(
            state.publish(vec![3], |_| Err("disk")),
            Err(PublishError::Write("disk"))
        );
        assert_eq!(
            state.restore::<_, ()>(|bytes| Ok(bytes.to_vec())).unwrap(),
            Some(vec![1, 2])
        );
    }
    #[test]
    fn decoder_failure_does_not_replace_saved_selection() {
        let state = SelectionBytes::loaded::<()>(Ok(Some(vec![1, 2]))).unwrap();
        assert_eq!(
            state.restore::<(), _>(|_| Err("invalid codec bytes")),
            Err("invalid codec bytes")
        );
        assert_eq!(
            state.restore::<_, ()>(|bytes| Ok(bytes.len())).unwrap(),
            Some(2)
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
