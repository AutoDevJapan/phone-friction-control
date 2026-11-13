//! Candidate app embedding ABI. Call only on the admitted provider's main
//! thread; pointers must originate here and cannot be used after release.
use crate::{
    ios_runtime::NativeSelectionSession,
    ios_selection_bridge::{NativeApi, Sink},
};
use std::{ffi::c_void, ptr::NonNull};

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn pfc_selection_session_open(
    path: *const u8,
    count: isize,
    api: *const NativeApi,
) -> *mut c_void {
    if path.is_null() || count <= 0 || api.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, count as usize) };
    let Ok(path) = std::str::from_utf8(bytes) else {
        return std::ptr::null_mut();
    };
    match unsafe { NativeSelectionSession::open(path.into(), *api) } {
        Ok(session) => Box::into_raw(Box::new(session)).cast(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn pfc_selection_session_restored(
    session: *const c_void,
    context: *mut c_void,
    sink: Option<Sink>,
) -> i32 {
    if session.is_null() {
        return -1;
    }
    let Some(sink) = sink else { return -1 };
    match unsafe { &*session.cast::<NativeSelectionSession>() }.encoded_selection() {
        Ok(None) => 1,
        Ok(Some(bytes)) => unsafe { sink(context, bytes.as_ptr(), bytes.len() as isize) },
        Err(_) => -2,
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn pfc_selection_session_commit(
    session: *mut c_void,
    selection: *mut c_void,
) -> i32 {
    // A nonnull session is required even on failure, so its provider can consume
    // the incoming ownership. Swift refuses closed callbacks before this call.
    if session.is_null() || selection.is_null() {
        return -1;
    }
    let session = unsafe { &mut *session.cast::<NativeSelectionSession>() };
    match unsafe { session.commit_picker_handle(NonNull::new(selection).unwrap()) } {
        Ok(()) => 0,
        Err(_) => -2,
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn pfc_selection_session_release(session: *mut c_void) {
    if !session.is_null() {
        drop(unsafe { Box::from_raw(session.cast::<NativeSelectionSession>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, io, path::PathBuf};

    // This provider exercises the Rust/C session contract, not Apple tokens or
    // Swift ownership. Each test thread owns its allocation/release counters.
    thread_local! {
        static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
        static FAIL_ENCODE: Cell<bool> = const { Cell::new(false) };
    }

    unsafe extern "C" fn decode(bytes: *const u8, count: isize) -> *mut c_void {
        if bytes.is_null() || count != 1 {
            return std::ptr::null_mut();
        }
        COUNTS.with(|state| {
            let (allocated, released) = state.get();
            state.set((allocated + 1, released));
        });
        Box::into_raw(Box::new(unsafe { *bytes })).cast()
    }

    unsafe extern "C" fn encode(handle: *mut c_void, context: *mut c_void, sink: Sink) -> i32 {
        if FAIL_ENCODE.with(Cell::get) {
            return -2;
        }
        unsafe { sink(context, handle.cast(), 1) }
    }

    unsafe extern "C" fn equal(a: *mut c_void, b: *mut c_void) -> i32 {
        i32::from(unsafe { *a.cast::<u8>() == *b.cast::<u8>() })
    }

    unsafe extern "C" fn release(handle: *mut c_void) {
        drop(unsafe { Box::from_raw(handle.cast::<u8>()) });
        COUNTS.with(|state| {
            let (allocated, released) = state.get();
            state.set((allocated, released + 1));
        });
    }

    #[derive(Default)]
    struct Received {
        calls: usize,
        bytes: Vec<u8>,
    }

    unsafe extern "C" fn receive(context: *mut c_void, bytes: *const u8, count: isize) -> i32 {
        let received = unsafe { &mut *context.cast::<Received>() };
        received.calls += 1;
        if bytes.is_null() || count != 1 {
            return -1;
        }
        received.bytes = unsafe { std::slice::from_raw_parts(bytes, count as usize) }.to_vec();
        0
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let base = std::env::var_os("PFC_SELECTION_TEST_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let path = base.join(format!(
                "ios-session-failure-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    struct OwnedSession(NonNull<c_void>);
    impl OwnedSession {
        fn new(raw: *mut c_void) -> Self {
            Self(NonNull::new(raw).expect("synthetic session failed to open"))
        }
        fn pointer(&self) -> *mut c_void {
            self.0.as_ptr()
        }
    }
    impl Drop for OwnedSession {
        fn drop(&mut self) {
            unsafe { pfc_selection_session_release(self.pointer()) };
        }
    }

    fn restored(session: *const c_void) -> (i32, Received) {
        let mut received = Received::default();
        let status = unsafe {
            pfc_selection_session_restored(
                session,
                (&mut received as *mut Received).cast(),
                Some(receive),
            )
        };
        (status, received)
    }

    #[test]
    fn failed_commit_consumes_handles_blocks_callbacks_and_reopens_previous_bytes() {
        COUNTS.with(|state| state.set((0, 0)));
        FAIL_ENCODE.with(|state| state.set(false));
        let fixture = Fixture::new();
        let path = fixture.0.join("selection.ron");
        let api = NativeApi {
            decode,
            encode,
            equal,
            release,
        };
        let open = || {
            let path = path.to_str().unwrap().as_bytes();
            unsafe { pfc_selection_session_open(path.as_ptr(), path.len() as isize, &api) }
        };
        let commit = |session, value: u8| unsafe {
            pfc_selection_session_commit(session, decode(&value, 1))
        };
        let session = OwnedSession::new(open());
        let (status, received) = restored(session.pointer());
        assert_eq!((status, received.calls), (1, 0));
        assert_eq!(commit(session.pointer(), 7), 0);
        assert_eq!(COUNTS.with(Cell::get), (2, 1));
        let saved = std::fs::read(&path).unwrap();
        let (status, received) = restored(session.pointer());
        assert_eq!((status, received.calls, received.bytes), (0, 1, vec![7]));

        FAIL_ENCODE.with(|state| state.set(true));
        assert_eq!(commit(session.pointer(), 9), -2);
        assert_eq!(COUNTS.with(Cell::get), (3, 3));
        FAIL_ENCODE.with(|state| state.set(false));
        let (status, received) = restored(session.pointer());
        assert_eq!((status, received.calls), (-2, 0));
        assert_eq!(commit(session.pointer(), 10), -2);
        assert_eq!(COUNTS.with(Cell::get), (4, 4));
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        drop(session);

        let session = OwnedSession::new(open());
        assert_eq!(COUNTS.with(Cell::get), (5, 4));
        let staging = path.with_extension("pending");
        std::fs::create_dir(&staging).unwrap();
        assert_eq!(commit(session.pointer(), 11), -2);
        assert_eq!(COUNTS.with(Cell::get), (7, 7));
        let (status, received) = restored(session.pointer());
        assert_eq!((status, received.calls), (-2, 0));
        assert_eq!(commit(session.pointer(), 12), -2);
        assert_eq!(COUNTS.with(Cell::get), (8, 8));
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        drop(session);
        std::fs::remove_dir(staging).unwrap();

        let session = OwnedSession::new(open());
        let (status, received) = restored(session.pointer());
        assert_eq!((status, received.calls, received.bytes), (0, 1, vec![7]));
        drop(session);
        assert_eq!(COUNTS.with(Cell::get), (9, 9));
        assert_eq!(std::fs::read(&path).unwrap(), saved);

        // Invalid stored codec bytes cannot become a default/open session and
        // cannot keep the ownership sidecar locked after open fails.
        std::fs::write(&path, "[1,2]").unwrap();
        assert!(open().is_null());
        assert_eq!(std::fs::read(&path).unwrap(), b"[1,2]");
        std::fs::write(&path, saved).unwrap();
        let session = OwnedSession::new(open());
        drop(session);
        assert_eq!(COUNTS.with(Cell::get), (10, 10));
        assert_eq!(
            io::ErrorKind::NotFound,
            std::fs::metadata(path.with_extension("pending"))
                .unwrap_err()
                .kind()
        );
    }
}
