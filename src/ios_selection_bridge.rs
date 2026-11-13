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
    #[test]
    fn provider_table_has_c_pointer_slots_in_declared_order() {
        use std::mem::{align_of, offset_of, size_of};
        type Entry = unsafe extern "C" fn();
        let slot = size_of::<Entry>();
        assert_eq!(size_of::<NativeApi>(), 4 * slot);
        assert_eq!(align_of::<NativeApi>(), align_of::<Entry>());
        assert_eq!(offset_of!(NativeApi, decode), 0);
        assert_eq!(offset_of!(NativeApi, encode), slot);
        assert_eq!(offset_of!(NativeApi, equal), 2 * slot);
        assert_eq!(offset_of!(NativeApi, release), 3 * slot);
    }

    #[test]
    fn encoding_error_keeps_handle_owned_and_each_drop_releases_once() {
        use std::cell::Cell;
        // Dedicated provider counters are thread-local: parallel tests cannot
        // turn another provider's releases into this test's lifetime evidence.
        thread_local! {
            static COUNTS: Cell<(usize, usize, usize, bool)> = const {
                Cell::new((0, 0, 0, true))
            };
        }
        unsafe extern "C" fn decode(bytes: *const u8, count: isize) -> *mut c_void {
            if bytes.is_null() || count != 1 {
                return std::ptr::null_mut();
            }
            COUNTS.with(|state| {
                let (allocated, released, encoded, fail) = state.get();
                state.set((allocated + 1, released, encoded, fail));
            });
            Box::into_raw(Box::new(unsafe { *bytes })).cast()
        }
        unsafe extern "C" fn encode(handle: *mut c_void, context: *mut c_void, sink: Sink) -> i32 {
            let fail = COUNTS.with(|state| {
                let (allocated, released, encoded, fail) = state.get();
                state.set((allocated, released, encoded + 1, fail));
                fail
            });
            if fail {
                return -2;
            }
            unsafe { sink(context, handle.cast(), 1) }
        }
        unsafe extern "C" fn release(handle: *mut c_void) {
            drop(unsafe { Box::from_raw(handle.cast::<u8>()) });
            COUNTS.with(|state| {
                let (allocated, released, encoded, fail) = state.get();
                state.set((allocated, released + 1, encoded, fail));
            });
        }
        let api = NativeApi {
            decode,
            encode,
            equal: equal_fixture,
            release,
        };
        {
            let codec = unsafe { NativeCodec::from_verified_api(api) };
            let selection = codec.decode(&[7]).unwrap();
            assert!(codec.encode(&selection).is_err());
            assert_eq!(COUNTS.with(Cell::get), (1, 0, 1, true));
            COUNTS.with(|state| state.set((1, 0, 1, false)));
            assert_eq!(codec.encode(&selection).unwrap(), [7]);
            assert_eq!(COUNTS.with(Cell::get), (1, 0, 2, false));
            drop(selection);
            assert_eq!(COUNTS.with(Cell::get), (1, 1, 2, false));

            let transferred = NonNull::new(unsafe { decode([9].as_ptr(), 1) }).unwrap();
            let adopted = unsafe { codec.adopt_owned_handle(transferred) };
            assert_eq!(COUNTS.with(Cell::get), (2, 1, 2, false));
            drop(adopted);
            assert_eq!(COUNTS.with(Cell::get), (2, 2, 2, false));
        }
        assert_eq!(COUNTS.with(Cell::get), (2, 2, 2, false));
    }
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
        let encoded_path = path.to_str().unwrap().as_bytes();
        let ffi = unsafe {
            crate::ios_selection_session_ffi::pfc_selection_session_open(
                encoded_path.as_ptr(),
                encoded_path.len() as isize,
                &api,
            )
        };
        assert!(!ffi.is_null());
        let mut transfer = Transfer {
            bytes: None,
            rejected: false,
        };
        assert_eq!(
            unsafe {
                crate::ios_selection_session_ffi::pfc_selection_session_restored(
                    ffi,
                    (&mut transfer as *mut Transfer).cast(),
                    Some(copy_bytes),
                )
            },
            1
        );
        let owned = unsafe { decode_fixture([7].as_ptr(), 1) };
        assert_eq!(
            unsafe { crate::ios_selection_session_ffi::pfc_selection_session_commit(ffi, owned) },
            0
        );
        assert_eq!(
            unsafe {
                crate::ios_selection_session_ffi::pfc_selection_session_restored(
                    ffi,
                    (&mut transfer as *mut Transfer).cast(),
                    Some(copy_bytes),
                )
            },
            0
        );
        assert_eq!(transfer.bytes, Some(vec![7]));
        unsafe { crate::ios_selection_session_ffi::pfc_selection_session_release(ffi) };
        std::fs::remove_file(&path).unwrap();
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
        let mut source = [7, 8];
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
