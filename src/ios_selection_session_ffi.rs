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
