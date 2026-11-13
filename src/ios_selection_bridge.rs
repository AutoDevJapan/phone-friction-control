//! Candidate consumer for SelectionBridge.h; not linked by the Runtime entry.
use crate::ios_selection_storage::SelectionCodec;
use std::{ffi::c_void, io, marker::PhantomData, ptr::NonNull, rc::Rc};

pub(crate) type Sink = unsafe extern "C" fn(*mut c_void, *const u8, isize) -> i32;

/// Function table is admitted only after the compiled C/Swift ABI is verified.
/// Supplying a table is unsafe: all entries must obey SelectionBridge.h.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct NativeApi {
    pub decode: unsafe extern "C" fn(*const u8, isize) -> *mut c_void,
    pub encode: unsafe extern "C" fn(*mut c_void, *mut c_void, Sink) -> i32,
    pub equal: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    pub release: unsafe extern "C" fn(*mut c_void),
}

impl NativeApi {
    fn same_provider(self, other: Self) -> bool {
        std::ptr::fn_addr_eq(self.decode, other.decode)
            && std::ptr::fn_addr_eq(self.encode, other.encode)
            && std::ptr::fn_addr_eq(self.equal, other.equal)
            && std::ptr::fn_addr_eq(self.release, other.release)
    }
}

pub(crate) struct NativeSelection {
    handle: NonNull<c_void>,
    api: NativeApi,
    // Swift handles and their release remain confined to the creating thread.
    _main_thread: PhantomData<Rc<()>>,
}

impl Drop for NativeSelection {
    fn drop(&mut self) {
        unsafe { (self.api.release)(self.handle.as_ptr()) }
    }
}

impl PartialEq for NativeSelection {
    fn eq(&self, other: &Self) -> bool {
        if !self.api.same_provider(other.api) {
            return false;
        }
        unsafe { (self.api.equal)(self.handle.as_ptr(), other.handle.as_ptr()) == 1 }
    }
}

pub(crate) struct NativeCodec {
    api: NativeApi,
    _main_thread: PhantomData<Rc<()>>,
}

impl NativeCodec {
    /// Caller must be on the main thread, and keep the verified provider image
    /// loaded for the entire lifetime of this codec and every returned handle.
    pub(crate) unsafe fn from_verified_api(api: NativeApi) -> Self {
        Self {
            api,
            _main_thread: PhantomData,
        }
    }

    /// Transfer one +1 handle from this exact admitted provider, on the main
    /// thread. The caller must not release or use that ownership afterward.
    pub(crate) unsafe fn adopt_owned_handle(&self, handle: NonNull<c_void>) -> NativeSelection {
        NativeSelection {
            handle,
            api: self.api,
            _main_thread: PhantomData,
        }
    }
}

struct Transfer {
    bytes: Option<Vec<u8>>,
    rejected: bool,
}

unsafe extern "C" fn copy_bytes(context: *mut c_void, bytes: *const u8, count: isize) -> i32 {
    if context.is_null() {
        return -1;
    }
    let transfer = unsafe { &mut *context.cast::<Transfer>() };
    if transfer.bytes.is_some() || count <= 0 || bytes.is_null() {
        transfer.rejected = true;
        return -1;
    }
    let count = count as usize;
    let mut owned = Vec::new();
    if owned.try_reserve_exact(count).is_err() {
        transfer.rejected = true;
        return -1;
    }
    // Provider guarantees readable bytes until the synchronous callback ends.
    owned.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, count) });
    transfer.bytes = Some(owned);
    0
}

impl SelectionCodec for NativeCodec {
    type Selection = NativeSelection;

    fn encode(&self, value: &NativeSelection) -> io::Result<Vec<u8>> {
        if !self.api.same_provider(value.api) {
            return Err(io::Error::other("Selection provider mismatch"));
        }
        let mut transfer = Transfer {
            bytes: None,
            rejected: false,
        };
        let status = unsafe {
            (self.api.encode)(
                value.handle.as_ptr(),
                (&mut transfer as *mut Transfer).cast(),
                copy_bytes,
            )
        };
        if status != 0 || transfer.rejected {
            return Err(io::Error::other("Selection encoding failed"));
        }
        transfer
            .bytes
            .ok_or_else(|| io::Error::other("Selection bytes missing"))
    }

    fn decode(&self, bytes: &[u8]) -> io::Result<NativeSelection> {
        let count = isize::try_from(bytes.len()).map_err(io::Error::other)?;
        if count == 0 {
            return Err(io::Error::other("Selection bytes empty"));
        }
        let handle = NonNull::new(unsafe { (self.api.decode)(bytes.as_ptr(), count) })
            .ok_or_else(|| io::Error::other("Selection decoding failed"))?;
        Ok(NativeSelection {
            handle,
            api: self.api,
            _main_thread: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" fn decode_fixture(bytes: *const u8, count: isize) -> *mut c_void {
        if count != 1 || bytes.is_null() {
            return std::ptr::null_mut();
        }
        Box::into_raw(Box::new(unsafe { *bytes })).cast()
    }
    unsafe extern "C" fn encode_fixture(
        handle: *mut c_void,
        context: *mut c_void,
        sink: Sink,
    ) -> i32 {
        unsafe { sink(context, handle.cast(), 1) }
    }
    unsafe extern "C" fn equal_fixture(a: *mut c_void, b: *mut c_void) -> i32 {
        i32::from(unsafe { *a.cast::<u8>() == *b.cast::<u8>() })
    }
    unsafe extern "C" fn release_fixture(handle: *mut c_void) {
        drop(unsafe { Box::from_raw(handle.cast::<u8>()) });
    }
    #[test]
    fn runtime_callsite_roundtrips_owned_provider_and_reopens_disk() {
        let api = NativeApi {
            decode: decode_fixture,
            encode: encode_fixture,
            equal: equal_fixture,
            release: release_fixture,
        };
        let root = std::env::temp_dir().join(format!(
            "ios-native-consumer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("selection.ron");
        // Synthetic provider implements this header's contract, not Apple ABI.
        let codec = unsafe { NativeCodec::from_verified_api(api) };
        let selected = codec.decode(&[7]).unwrap();
        let mut runtime =
            unsafe { crate::ios_runtime::NativeSelectionSession::open(path.clone(), api) }.unwrap();
        assert!(runtime.selection().is_none());
        let transferred = NonNull::new(unsafe { decode_fixture([7].as_ptr(), 1) }).unwrap();
        unsafe { runtime.commit_picker_handle(transferred) }.unwrap();
        assert!(runtime.selection().is_some_and(|value| value == &selected));
        drop(runtime);
        let runtime =
            unsafe { crate::ios_runtime::NativeSelectionSession::open(path.clone(), api) }.unwrap();
        assert!(runtime.selection().is_some_and(|value| value == &selected));
        drop(runtime);
        std::fs::remove_file(&path).unwrap();
        let (mut session, initial) =
            unsafe { crate::ios_runtime::open_native_selection(path.clone(), api) }.unwrap();
        assert!(initial.is_none());
        session.commit(&selected).unwrap();
        drop(session);
        let (session, restored) =
            unsafe { crate::ios_runtime::open_native_selection(path.clone(), api) }.unwrap();
        assert!(restored.as_ref().is_some_and(|value| value == &selected));
        drop(restored);
        drop(selected);
        drop(session);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(root.join("selection.ron.owner-lock")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn transfer_copies_and_rejects_duplicate_or_missing_bytes() {
        let mut transfer = Transfer {
            bytes: None,
            rejected: false,
        };
        let mut source = vec![7, 8];
        let context = (&mut transfer as *mut Transfer).cast();
        assert_eq!(unsafe { copy_bytes(context, source.as_ptr(), 2) }, 0);
        source[0] = 9;
        assert_eq!(transfer.bytes.as_deref(), Some(&[7, 8][..]));
        assert_eq!(unsafe { copy_bytes(context, source.as_ptr(), 2) }, -1);
        assert!(transfer.rejected);
        let mut missing = Transfer {
            bytes: None,
            rejected: false,
        };
        assert_eq!(
            unsafe { copy_bytes((&mut missing as *mut Transfer).cast(), std::ptr::null(), 1) },
            -1
        );
        assert!(missing.rejected && missing.bytes.is_none());
    }
}
