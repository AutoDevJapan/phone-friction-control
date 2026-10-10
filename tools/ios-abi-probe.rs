//! Compiler experiment only: no Apple framework is linked or executed.
//! The undefined external symbol deliberately prevents this from being an app.
#![no_std]
#![feature(abi_swift)]

unsafe extern "Swift" {
    fn abi_probe_external();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_calls_swift_probe() {
    unsafe { abi_probe_external() };
}
