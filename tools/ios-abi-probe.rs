//! Default mode: compiler experiment with an undefined external symbol.
//! runtime_probe mode: isolated Simulator getter and async runtime inspection.
//! Authorization requests require the explicit auth_request mode.
#![cfg_attr(not(any(runtime_probe, ui_test_bundle)), no_std)]
#![cfg_attr(not(ui_test_bundle), feature(abi_swift))]
#![cfg_attr(xcode_host, no_main)]

#[cfg(all(authorization_status_entry_probe, not(runtime_probe)))]
compile_error!("status entry inspection requires the isolated runtime probe");
#[cfg(all(
    authorization_status_value_probe,
    not(authorization_status_entry_probe)
))]
compile_error!("status value reading requires the verified status entry mode");
#[cfg(all(
    authorization_status_entry_probe,
    any(auth_request, uikit_host, xcode_host, ui_test_bundle)
))]
compile_error!("status entry inspection is a separate request-free process");

#[cfg(all(
    authorization_status_consent_probe,
    not(all(runtime_probe, ui_test_host, uikit_host, xcode_host, auth_request))
))]
compile_error!("consent status reading requires the isolated Xcode UI test host");

#[cfg(all(
    ui_test_bundle,
    not(all(target_os = "ios", target_arch = "aarch64", target_abi = "sim"))
))]
compile_error!("the permission UI fixture is limited to arm64 iOS Simulator");

#[cfg(all(
    ui_test_host,
    not(all(runtime_probe, uikit_host, auth_request, xcode_host))
))]
compile_error!("the UI test host requires the Xcode UIKit authorization mode");
#[cfg(all(ui_test_continue, not(ui_test_bundle)))]
compile_error!("the Continue test mode requires the isolated XCTest bundle");
#[cfg(all(ui_test_bundle, runtime_probe))]
compile_error!("the XCTest bundle and inspected application are separate products");

#[cfg(all(
    runtime_probe,
    not(all(target_os = "ios", target_arch = "aarch64", target_abi = "sim"))
))]
compile_error!("runtime inspection is limited to arm64 iOS Simulator");

#[cfg(not(any(runtime_probe, ui_test_bundle)))]
unsafe extern "Swift" {
    fn abi_probe_external();
}

#[unsafe(no_mangle)]
#[cfg(not(any(runtime_probe, ui_test_bundle)))]
pub unsafe extern "C" fn rust_calls_swift_probe() {
    unsafe { abi_probe_external() };
}

#[cfg(runtime_probe)]
#[repr(C)]
struct MetadataResponse {
    value: *const std::ffi::c_void,
    state: usize,
}

#[cfg(runtime_probe)]
#[link(name = "FamilyControls", kind = "framework")]
unsafe extern "Swift" {
    // Non-generic accessor: MetadataRequest -> MetadataResponse (Swift 5.10 ABI).
    #[link_name = env!("AUTH_METADATA_SYMBOL")]
    fn authorization_center_metadata(request: usize) -> MetadataResponse;
}

// A class static getter receives its metatype through swiftself, which uses x20
// on Darwin arm64. This Rust-owned assembly boundary preserves the C caller's
// callee-saved x20 and return address. It makes no type-layout assumptions about
// the Swift object beyond its documented single class-reference pointer.
#[cfg(runtime_probe)]
core::arch::global_asm!(concat!(
    ".text\n.p2align 2\n.globl _rust_shared_with_metadata\n",
    "_rust_shared_with_metadata:\n.cfi_startproc\n",
    "stp x20, x30, [sp, #-16]!\n.cfi_def_cfa_offset 16\n",
    ".cfi_offset 20, -16\n.cfi_offset 30, -8\nmov x20, x0\n",
    "bl _",
    env!("AUTH_SHARED_SYMBOL"),
    "\nldp x20, x30, [sp], #16\n.cfi_restore 20\n.cfi_restore 30\n",
    ".cfi_def_cfa_offset 0\nret\n.cfi_endproc\n",
));

#[cfg(runtime_probe)]
unsafe extern "C" {
    fn rust_shared_with_metadata(metadata: *const std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(runtime_probe)]
#[link(name = "objc")]
unsafe extern "C" {
    fn object_getClass(object: *mut std::ffi::c_void) -> *const std::ffi::c_void;
    fn class_getName(class: *const std::ffi::c_void) -> *const std::ffi::c_char;
}

#[cfg(runtime_probe)]
fn inspect_singleton() -> *mut std::ffi::c_void {
    // This deliberately does not request authorization or read another app's data.
    // Request 0 means a blocking request for complete metadata.
    let metadata = unsafe { authorization_center_metadata(0) };
    assert_eq!(metadata.state, 0, "SDK class metadata is incomplete");
    assert!(!metadata.value.is_null(), "SDK class metadata is null");
    let object = unsafe { rust_shared_with_metadata(metadata.value) };
    assert!(
        !object.is_null(),
        "SDK getter returned a null class reference"
    );
    let class = unsafe { object_getClass(object) };
    assert!(
        !class.is_null(),
        "SDK reference has no Objective-C runtime class"
    );
    assert_eq!(
        class, metadata.value,
        "SDK getter returned a reference of a different runtime class"
    );
    let name = unsafe { class_getName(class) };
    assert!(!name.is_null(), "SDK runtime class has no name");
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_str().unwrap();
    println!("SDK singleton class: {name}");
    object
}

#[cfg(runtime_probe)]
#[cfg(not(xcode_host))]
fn main() {
    #[cfg(uikit_host)]
    native_host::run();
    #[cfg(not(uikit_host))]
    {
        let object = inspect_singleton();
        #[cfg(authorization_status_entry_probe)]
        {
            let metadata = authorization_status_entry::inspect(object);
            #[cfg(authorization_status_value_probe)]
            {
                let status = authorization_status_entry::read_value(object, metadata);
                println!(
                    "Authorization status value read: raw_value={}; label={}; status_getter_calls=1; raw_value_getter_calls=1; enum_case_name_calls=1; status_value_verified=true; authorization_requests=0; grant_verified=false",
                    status.raw, status.label
                );
            }
            #[cfg(not(authorization_status_value_probe))]
            let _ = metadata;
        }
        #[cfg(not(authorization_status_entry_probe))]
        async_inspection::inspect();
        #[cfg(auth_request)]
        request_probe::run(object);
        #[cfg(not(any(authorization_status_entry_probe, auth_request)))]
        let _ = object;
    }
}

#[cfg(any(authorization_status_entry_probe, authorization_status_consent_probe))]
mod authorization_status_entry {
    use super::MetadataResponse;
    use std::ffi::{CString, c_char, c_void};

    #[link(name = "System")]
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn getpagesize() -> i32;
        fn dladdr(address: *const c_void, information: *mut DlInfo) -> i32;
    }
    #[repr(C)]
    struct DlInfo {
        filename: *const c_char,
        base: *const c_void,
        symbol: *const c_char,
        symbol_address: *const c_void,
    }
    fn resolve(symbol: &str) -> *const c_void {
        let name = CString::new(symbol).unwrap();
        let entry = unsafe {
            dlsym(
                std::ptr::without_provenance_mut(usize::MAX - 1),
                name.as_ptr(),
            )
        };
        assert!(
            !entry.is_null() && entry.addr() % 4 == 0,
            "invalid public status entry"
        );
        entry
    }
    unsafe fn read<T: Copy>(pointer: *const T, page: usize) -> T {
        assert!(
            !pointer.is_null() && pointer.addr() % std::mem::align_of::<T>() == 0,
            "invalid status ABI field"
        );
        assert!(
            page - pointer.addr() % page >= std::mem::size_of::<T>(),
            "status ABI field crosses page"
        );
        unsafe { pointer.read() }
    }
    fn prefix(label: &str, entry: *const c_void, page: usize) {
        assert!(
            !entry.is_null() && entry.addr() % 4 == 0,
            "invalid status code entry"
        );
        let length = 128.min(page - entry.addr() % page);
        let bytes = unsafe { std::slice::from_raw_parts(entry.cast::<u8>(), length) };
        print!("Authorization status {label}: length={length}; hex=");
        for byte in bytes {
            print!("{byte:02x}");
        }
        println!();
    }
    pub(super) fn inspect(object: *mut c_void) -> *const c_void {
        let page = unsafe { getpagesize() };
        assert!(page > 0, "invalid page size");
        let page = page as usize;
        let accessor: unsafe extern "Swift" fn(usize) -> MetadataResponse =
            unsafe { std::mem::transmute(resolve(env!("AUTH_STATUS_METADATA_SYMBOL"))) };
        let response = unsafe { accessor(0) };
        assert!(
            response.state == 0 && !response.value.is_null(),
            "incomplete status metadata"
        );
        assert!(
            unsafe { read(response.value.cast::<usize>(), page) } == 0x201,
            "expected enum metadata"
        );
        let witnesses =
            unsafe { read(response.value.cast::<*const c_void>().wrapping_sub(1), page) };
        assert!(!witnesses.is_null(), "missing status witnesses");
        // Swift 5.10 required VWT: eight function pointers, size, stride,
        // uint32 flags and extra inhabitant count. No witness is invoked here.
        let words = witnesses.cast::<usize>();
        let size = unsafe { read(words.wrapping_add(8), page) };
        let stride = unsafe { read(words.wrapping_add(9), page) };
        let flags = unsafe { read(words.wrapping_add(10).cast::<u32>(), page) };
        let alignment = (flags as usize & 0xff) + 1;
        assert!(
            flags & 0x0040_0000 == 0 && alignment.is_power_of_two(),
            "incomplete status layout"
        );
        assert!(
            size > 0 && size <= 64 && stride >= size && stride % alignment == 0,
            "invalid status value size"
        );
        println!(
            "Authorization status layout: size={size}; stride={stride}; alignment={alignment}; metadata_complete=true; kind_enum=true"
        );
        let getter = resolve(env!("AUTH_STATUS_GET_SYMBOL"));
        prefix("getter entry", getter, page);
        // Actual SDK17.5 public dispatch thunk (run38077790004):
        // ldr x9,[x20]; ldr x0,[x9,#slot]; br x0. Validate that exact
        // instruction shape before deriving the slot; never invoke either entry.
        let instructions = getter.cast::<u32>();
        assert!(
            unsafe { read(instructions, page) } == 0xf940_0289,
            "getter class load changed"
        );
        let load = unsafe { read(instructions.wrapping_add(1), page) };
        assert!(
            load & 0xffc0_03ff == 0xf940_0120,
            "getter slot load changed"
        );
        assert!(
            unsafe { read(instructions.wrapping_add(2), page) } == 0xd61f_0000,
            "getter branch changed"
        );
        let slot = ((load >> 10) & 0xfff) as usize * 8;
        let class = unsafe { super::object_getClass(object) };
        assert!(!class.is_null(), "status center class missing");
        assert!(
            unsafe { read(object.cast::<*const c_void>(), page) } == class,
            "getter requires an unpacked Swift class reference"
        );
        let target = unsafe {
            read(
                class
                    .cast::<u8>()
                    .wrapping_add(slot)
                    .cast::<*const c_void>(),
                page,
            )
        };
        let info = || DlInfo {
            filename: std::ptr::null(),
            base: std::ptr::null(),
            symbol: std::ptr::null(),
            symbol_address: std::ptr::null(),
        };
        let mut source_info = info();
        let mut target_info = info();
        assert!(
            unsafe { dladdr(getter, &mut source_info) } != 0
                && unsafe { dladdr(target, &mut target_info) } != 0
                && !source_info.base.is_null()
                && source_info.base == target_info.base,
            "status getter implementation image differs"
        );
        prefix("getter implementation", target, page);
        prefix(
            "raw value getter entry",
            resolve(env!("AUTH_STATUS_RAW_GET_SYMBOL")),
            page,
        );
        println!(
            "Authorization status dispatch observed: same_image=true; slot_derived=true; implementation_calls=0; raw_value_getter_calls=0"
        );
        #[cfg(authorization_status_entry_probe)]
        println!(
            "Authorization status entry inspection complete; phase=entry; status_getter_calls=0; enum_values_constructed=0; value_witness_calls=0; authorization_requests=0; grant_verified=false"
        );
        response.value
    }

    // Actual entry2 evidence: status implementation saves x8 (indirect enum
    // result); public rawValue getter is ldrb w0,[x20]; ret. Preserve C's x20.
    #[cfg(any(authorization_status_value_probe, authorization_status_consent_probe))]
    core::arch::global_asm!(
        ".text",
        ".p2align 2",
        ".globl _rust_status_with_self",
        "_rust_status_with_self:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x20, x0",
        "mov x8, x1",
        "blr x2",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
        ".p2align 2",
        ".globl _rust_status_raw_with_self",
        "_rust_status_raw_with_self:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x20, x0",
        "blr x1",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
    );
    #[cfg(any(authorization_status_value_probe, authorization_status_consent_probe))]
    unsafe extern "C" {
        fn rust_status_with_self(object: *mut c_void, result: *mut c_void, entry: *const c_void);
        fn rust_status_raw_with_self(value: *mut c_void, entry: *const c_void) -> isize;
    }
    #[cfg(any(authorization_status_value_probe, authorization_status_consent_probe))]
    #[link(name = "swiftCore")]
    unsafe extern "Swift" {
        // Swift5.10 ReflectionMirror.cpp: SWIFT_CC(swift), two explicit args,
        // borrowed pointer to the enum case name. Exact SDK export is checked.
        #[link_name = "swift_EnumCaseName"]
        fn enum_case_name(value: *mut c_void, metadata: *const c_void) -> *const c_char;
    }
    #[cfg(any(authorization_status_value_probe, authorization_status_consent_probe))]
    pub(super) struct StatusValue {
        pub(super) raw: isize,
        pub(super) label: &'static str,
    }
    #[cfg(any(authorization_status_value_probe, authorization_status_consent_probe))]
    pub(super) fn read_value(object: *mut c_void, metadata: *const c_void) -> StatusValue {
        let page = unsafe { getpagesize() } as usize;
        let table = unsafe { read(metadata.cast::<*const c_void>().wrapping_sub(1), page) };
        let words = table.cast::<usize>();
        let size = unsafe { read(words.wrapping_add(8), page) };
        let stride = unsafe { read(words.wrapping_add(9), page) };
        let flags = unsafe { read(words.wrapping_add(10).cast::<u32>(), page) };
        // This specific SDK enum is one-byte POD; no owned payload to destroy.
        assert!(
            size == 1 && stride == 1 && flags & 0x100ff == 0,
            "status value ABI changed"
        );
        let raw_getter = resolve(env!("AUTH_STATUS_RAW_GET_SYMBOL"));
        assert!(
            unsafe { read(raw_getter.cast::<u32>(), page) } == 0x3940_0280
                && unsafe { read(raw_getter.cast::<u32>().wrapping_add(1), page) } == 0xd65f_03c0,
            "rawValue getter convention changed"
        );
        let mut value = std::mem::MaybeUninit::new(0xff_u8);
        unsafe {
            rust_status_with_self(
                object,
                value.as_mut_ptr().cast(),
                resolve(env!("AUTH_STATUS_GET_SYMBOL")),
            )
        };
        let raw = unsafe { rust_status_raw_with_self(value.as_mut_ptr().cast(), raw_getter) };
        assert!((0..=2).contains(&raw), "invalid public status raw value");
        let name = unsafe { enum_case_name(value.as_mut_ptr().cast(), metadata) };
        assert!(!name.is_null(), "status case reflection unavailable");
        let count = 32.min(page - name.addr() % page);
        let mut bytes = Vec::new();
        let mut terminated = false;
        for offset in 0..count {
            let byte = unsafe { name.cast::<u8>().add(offset).read() };
            if byte == 0 {
                terminated = true;
                break;
            }
            bytes.push(byte);
        }
        assert!(terminated, "status name exceeds bound");
        let label = std::str::from_utf8(&bytes).unwrap();
        assert!(
            matches!(
                (raw, label),
                (0, "notDetermined") | (1, "denied") | (2, "approved")
            ),
            "status case/raw mapping differs"
        );
        let label = match raw {
            0 => "notDetermined",
            1 => "denied",
            2 => "approved",
            _ => unreachable!(),
        };
        StatusValue { raw, label }
    }
}

#[cfg(all(uikit_host, not(auth_request)))]
compile_error!("UIKit host inspection requires auth_request");

#[cfg(all(runtime_probe, xcode_host, uikit_host, auth_request))]
#[unsafe(no_mangle)]
pub extern "C" fn main(_: i32, _: *mut *mut std::ffi::c_char) -> i32 {
    native_host::run();
    1
}

#[cfg(all(xcode_host, not(all(runtime_probe, uikit_host, auth_request))))]
compile_error!("Xcode static library host requires the UIKit authorization mode");

#[cfg(all(
    any(
        selection_layout_probe,
        binding_entry_probe,
        selection_value_probe,
        mutable_binding_probe
    ),
    any(
        ui_test_host,
        ui_test_bundle,
        ui_test_continue,
        authorization_status_entry_probe,
        authorization_status_value_probe,
        authorization_status_consent_probe
    )
))]
compile_error!("selection inspection cannot be combined with UI or status inspection");
#[cfg(all(binding_entry_probe, selection_value_probe))]
compile_error!("choose either selection entry inspection or value lifecycle");
#[cfg(all(
    selection_layout_probe,
    not(all(runtime_probe, uikit_host, auth_request, xcode_host))
))]
compile_error!("selection layout inspection requires the Xcode UIKit authorization host");
#[cfg(all(binding_entry_probe, not(selection_layout_probe)))]
compile_error!("binding entry inspection requires the selection layout mode");
#[cfg(all(selection_value_probe, not(selection_layout_probe)))]
compile_error!("selection value lifecycle requires the selection layout mode");
#[cfg(all(mutable_binding_probe, not(selection_layout_probe)))]
compile_error!("mutable binding inspection requires the selection layout mode");
#[cfg(all(mutable_binding_probe, any(binding_entry_probe, selection_value_probe)))]
compile_error!("mutable binding inspection is a separate finite value experiment");

// Layout and entry-byte evidence only; no Selection or Binding value is built.
#[cfg(selection_layout_probe)]
mod selection_layout {
    use super::MetadataResponse;
    use std::ffi::{CString, c_char, c_void};

    // Swift 5.10 ABI/ValueWitness.def (required witnesses), in declaration order:
    // initializeBufferWithCopyOfBuffer, destroy, initializeWithCopy,
    // assignWithCopy, initializeWithTake, assignWithTake,
    // getEnumTagSinglePayload, storeEnumTagSinglePayload, then the data fields.
    // ABI/ValueWitnessTable.h uses size_t, size_t, uint32_t, uint32_t.
    #[repr(C)]
    pub(super) struct ValueWitnessTable {
        pub(super) functions: [*const c_void; 8],
        pub(super) size: usize,
        pub(super) stride: usize,
        pub(super) flags: u32,
        _extra_inhabitant_count: u32,
    }

    // Runtime/HeapObject.h: Swift CC returns object then uninitialized buffer.
    #[repr(C)]
    struct BoxPair {
        object: *mut c_void,
        buffer: *mut c_void,
    }
    #[link(name = "FamilyControls", kind = "framework")]
    #[link(name = "SwiftUI", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "swiftCore")]
    unsafe extern "Swift" {
        fn swift_allocBox(metadata: *const c_void) -> BoxPair;
    }
    #[link(name = "swiftCore")]
    unsafe extern "C" {
        // These declarations have no SWIFT_CC(swift) in HeapObject.h.
        fn swift_projectBox(object: *mut c_void) -> *mut c_void;
        fn swift_deallocBox(object: *mut c_void);
    }
    #[link(name = "System")]
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn getpagesize() -> i32;
        #[cfg(binding_entry_probe)]
        fn dladdr(address: *const c_void, information: *mut DlInfo) -> i32;
    }
    #[cfg(binding_entry_probe)]
    #[repr(C)]
    struct DlInfo {
        filename: *const c_char,
        base: *const c_void,
        symbol_name: *const c_char,
        symbol_address: *const c_void,
    }
    struct UninitializedBox(*mut c_void);
    impl Drop for UninitializedBox {
        fn drop(&mut self) {
            // Never release an uninitialized value through its destroy witness.
            unsafe { swift_deallocBox(self.0) };
        }
    }

    pub(super) fn resolve(symbol: &str) -> *const c_void {
        let name = CString::new(symbol).unwrap();
        // Darwin dyld include/dlfcn.h: RTLD_DEFAULT is ((void *) -2).
        // Only the four exact SDK symbols checked by the workflow are looked up.
        let pointer = unsafe {
            dlsym(
                std::ptr::without_provenance_mut(usize::MAX - 1),
                name.as_ptr(),
            )
        };
        assert!(
            !pointer.is_null(),
            "required public selection symbol is missing"
        );
        assert!(pointer.addr() % 4 == 0, "unaligned arm64 public entry");
        pointer
    }

    fn page_remaining(pointer: *const c_void, page: usize) -> usize {
        assert!(!pointer.is_null(), "null inspection address");
        page - pointer.addr() % page
    }
    pub(super) unsafe fn read<T: Copy>(pointer: *const T, page: usize) -> T {
        assert!(
            pointer.addr() % std::mem::align_of::<T>() == 0,
            "unaligned ABI field"
        );
        assert!(
            page_remaining(pointer.cast(), page) >= std::mem::size_of::<T>(),
            "ABI field crosses page boundary"
        );
        unsafe { pointer.read() }
    }
    fn entry_bytes(label: &str, pointer: *const c_void, page: usize) {
        // This is a bounded entry prefix, not a claim about argument registers,
        // ownership, or a whole function. Do not follow branches or call it.
        let count = 128.min(page_remaining(pointer, page));
        let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), count) };
        print!("Selection entry bytes: {label}; length={count}; hex=");
        for byte in bytes {
            print!("{byte:02x}");
        }
        println!();
    }
    #[cfg(binding_entry_probe)]
    fn control_transfer_prefixes(label: &str, entry: *const c_void, page: usize) {
        // Inspect a bounded straight-line prefix, stopping at its first branch,
        // indirect call or return. A getter may forward with B; a setter may
        // call indirectly. Bytes after those edges are not its executed prefix.
        // Direct destinations are observed once, never invoked or recursively followed.
        let count = 128.min(page_remaining(entry, page));
        for offset in (0..count.saturating_sub(3)).step_by(4) {
            let instruction = unsafe { read(entry.cast::<u8>().add(offset).cast::<u32>(), page) };
            let direct_call = instruction & 0xfc00_0000 == 0x9400_0000;
            let direct_branch = instruction & 0xfc00_0000 == 0x1400_0000;
            let indirect_call = instruction & 0xffff_fc1f == 0xd63f_0000;
            let indirect_branch = instruction & 0xffff_fc1f == 0xd61f_0000;
            let returned = instruction & 0xffff_fc1f == 0xd65f_0000;
            let conditional = instruction & 0xff00_0010 == 0x5400_0000
                || instruction & 0x7e00_0000 == 0x3400_0000
                || instruction & 0x7e00_0000 == 0x3600_0000;
            if indirect_call || indirect_branch || returned || conditional {
                let kind = if indirect_call {
                    "indirect_call"
                } else if indirect_branch {
                    "indirect_branch"
                } else if returned {
                    "return"
                } else {
                    "conditional_branch"
                };
                println!(
                    "Binding control transfer: {label}; source_offset={offset}; kind={kind}; target_read=false"
                );
                return;
            }
            if !direct_call && !direct_branch {
                continue;
            }
            let kind = if direct_call {
                "direct_call"
            } else {
                "direct_branch"
            };
            let relative = ((instruction << 6) as i32 >> 6) as isize * 4;
            let target_address = entry
                .addr()
                .checked_add(offset)
                .and_then(|address| address.checked_add_signed(relative))
                .expect("BL address overflow");
            let displacement = (offset as isize)
                .checked_add(relative)
                .expect("BL displacement overflow");
            let target = entry
                .cast::<u8>()
                .wrapping_offset(displacement)
                .cast::<c_void>();
            assert!(
                target.addr() == target_address,
                "BL address projection mismatch"
            );
            let empty = || DlInfo {
                filename: std::ptr::null(),
                base: std::ptr::null(),
                symbol_name: std::ptr::null(),
                symbol_address: std::ptr::null(),
            };
            let mut source_info = empty();
            let mut target_info = empty();
            assert!(
                unsafe { dladdr(entry, &mut source_info) } != 0,
                "public entry image missing"
            );
            if unsafe { dladdr(target, &mut target_info) } != 0
                && !source_info.base.is_null()
                && source_info.base == target_info.base
            {
                println!(
                    "Binding control transfer: {label}; source_offset={offset}; kind={kind}; relative_bytes={relative}; same_image=true"
                );
                entry_bytes(
                    &format!("{label}.offset{offset} {kind} target"),
                    target,
                    page,
                );
            } else {
                println!(
                    "Binding control transfer: {label}; source_offset={offset}; kind={kind}; skipped_cross_image=true"
                );
            }
            if direct_branch {
                return;
            }
        }
        println!("Binding control transfer: {label}; straight_line_prefix_end=true");
    }

    fn layout(label: &str, response: MetadataResponse, page: usize) -> usize {
        assert!(response.state == 0, "incomplete selection metadata");
        assert!(!response.value.is_null(), "null selection metadata");
        let metadata = response.value.cast::<usize>();
        // Swift 5.10 ABI/MetadataKind.def: Struct = MetadataKindIsNonHeap,
        // ABI/MetadataValues.h: MetadataKindIsNonHeap = 0x200.
        assert!(
            unsafe { read(metadata, page) } == 0x200,
            "expected struct metadata"
        );
        // ABI/Metadata.h: FullMetadata inherits HeaderType before Metadata;
        // HeaderType's final word is ValueWitnesses. The layout-string prefix
        // is before that word, so VWT remains at canonical metadata minus one.
        let witnesses = unsafe {
            read(
                metadata.cast::<*const ValueWitnessTable>().wrapping_sub(1),
                page,
            )
        };
        assert!(!witnesses.is_null(), "null selection value witnesses");
        let size = unsafe { read(&raw const (*witnesses).size, page) };
        let stride = unsafe { read(&raw const (*witnesses).stride, page) };
        let flags = unsafe { read(&raw const (*witnesses).flags, page) };
        // MetadataValues.h: AlignmentMask=0xff, Incomplete=0x00400000.
        assert!(flags & 0x0040_0000 == 0, "incomplete value layout");
        let alignment = (flags as usize & 0xff) + 1;
        assert!(alignment.is_power_of_two(), "invalid value alignment");
        let expected_stride = size.checked_add(alignment - 1).unwrap() & !(alignment - 1);
        assert!(stride == expected_stride.max(1), "invalid value stride");
        println!(
            "Selection layout: {label}; size={size}; stride={stride}; alignment={alignment}; metadata_complete=true; kind_struct=true"
        );
        // Slots 2 and 3 are initializeWithCopy and assignWithCopy. Read only
        // their addresses; no VWT function is called and no value is accessed.
        for (slot, name) in [(2, "initializeWithCopy"), (3, "assignWithCopy")] {
            let entry = unsafe {
                read(
                    (&raw const (*witnesses).functions)
                        .cast::<*const c_void>()
                        .wrapping_add(slot),
                    page,
                )
            };
            assert!(entry.addr() % 4 == 0, "unaligned arm64 value witness entry");
            entry_bytes(&format!("{label}.{name}"), entry, page);
        }
        alignment
    }

    pub(super) fn inspect() -> *const c_void {
        let page = unsafe { getpagesize() };
        assert!(page > 0, "invalid Darwin page size");
        let page = page as usize;
        // ABI/Metadata.h MetadataAccessFunction::operator(): Fn0 and Fn1
        // both use Swift CC, with request before the direct generic argument.
        // SDK Binding's <Value> has no conformance requirements.
        let selection_accessor: unsafe extern "Swift" fn(usize) -> MetadataResponse =
            unsafe { std::mem::transmute(resolve(env!("SELECTION_METADATA_SYMBOL"))) };
        let binding_accessor: unsafe extern "Swift" fn(usize, *const c_void) -> MetadataResponse =
            unsafe { std::mem::transmute(resolve(env!("BINDING_METADATA_SYMBOL"))) };
        // MetadataRequest(0) requests complete metadata synchronously.
        let selection = unsafe { selection_accessor(0) };
        let selection_pointer = selection.value;
        let alignment = layout("FamilyActivitySelection", selection, page);
        let binding = unsafe { binding_accessor(0, selection_pointer) };
        layout("Binding<FamilyActivitySelection>", binding, page);

        let pair = unsafe { swift_allocBox(selection_pointer) };
        assert!(!pair.object.is_null(), "null selection box object");
        let owner = UninitializedBox(pair.object);
        assert!(
            pair.object.addr() % std::mem::align_of::<*mut c_void>() == 0,
            "unaligned box object"
        );
        assert!(!pair.buffer.is_null(), "null selection box buffer");
        assert!(
            pair.buffer.addr() % alignment == 0,
            "unaligned selection box buffer"
        );
        let projected = unsafe { swift_projectBox(owner.0) };
        assert!(
            !projected.is_null() && projected == pair.buffer,
            "box projection mismatch"
        );
        drop(owner);
        println!(
            "Selection box: buffer_aligned=true; project_matches=true; uninitialized_deallocated=true"
        );
        entry_bytes(
            "FamilyActivitySelection.init()",
            resolve(env!("SELECTION_INIT_SYMBOL")),
            page,
        );
        entry_bytes(
            "Binding.init(get:set:) without Transaction",
            resolve(env!("BINDING_INIT_SYMBOL")),
            page,
        );
        #[cfg(binding_entry_probe)]
        {
            let constructor = resolve(env!("BINDING_INIT_SYMBOL"));
            let getter = resolve(env!("BINDING_GET_SYMBOL"));
            let setter = resolve(env!("BINDING_SET_SYMBOL"));
            entry_bytes("Binding.wrappedValue getter", getter, page);
            entry_bytes("Binding.wrappedValue setter", setter, page);
            for (label, entry) in [
                ("Binding.init", constructor),
                ("Binding.get", getter),
                ("Binding.set", setter),
            ] {
                control_transfer_prefixes(label, entry, page);
            }
            println!(
                "Binding accessor inspection complete; helper_calls=0; getter_calls=0; setter_calls=0; closure_calls=0; ownership_verified=false"
            );
        }
        println!(
            "Selection layout inspection complete; phase=layout; selection_values_constructed=0; selection_values_read=0; initializer_calls=0; closure_calls=0; value_witness_calls=0; authorization_requests=0; authorization_status_verified=false; monitoring=false; shielding=false"
        );
        selection_pointer
    }
}

// Separate prerequisite experiment: real default values, never a mutable Binding.
#[cfg(all(selection_value_probe, selection_layout_probe))]
mod selection_value {
    use super::selection_layout::{ValueWitnessTable, read, resolve};
    use std::alloc::{Layout, alloc_zeroed, dealloc};
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::ptr::NonNull;

    // SDK 17.5 entry_4 (binding-control-prefix-disassembly.txt): init() has no
    // parameters and returns its resilient value indirectly in x8. Do not model
    // its fields. C receives (destination, dynamically resolved public entry).
    // Preserve C/Swift callee-saved registers and the return address; the Swift
    // call may clobber x8 and all caller-saved registers. No swiftself is needed.
    core::arch::global_asm!(
        ".text",
        ".p2align 2",
        ".globl _rust_selection_default_init",
        "_rust_selection_default_init:",
        ".cfi_startproc",
        "stp x29, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 29, -16",
        ".cfi_offset 30, -8",
        "mov x29, sp",
        "mov x8, x0",
        "blr x1",
        "ldp x29, x30, [sp], #16",
        ".cfi_restore 29",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
    );
    unsafe extern "C" {
        fn rust_selection_default_init(destination: *mut c_void, entry: *const c_void);
        fn getpagesize() -> i32;
    }
    // Swift 5.10 ValueWitness.def/Table.h: required slots 1, 2, 3. The copy
    // signatures use mutable opaque pointers even though the source is preserved.
    type Destroy = unsafe extern "Swift" fn(*mut c_void, *const c_void);
    type Copy = unsafe extern "Swift" fn(*mut c_void, *mut c_void, *const c_void) -> *mut c_void;
    struct Witnesses {
        metadata: *const c_void,
        destroy: Destroy,
        initialize: Copy,
        assign: Copy,
        layout: Layout,
        size: usize,
    }
    #[derive(Default)]
    struct Counts {
        allocations: usize,
        deallocations: usize,
        initializer_calls: usize,
        initialize_with_copy_calls: usize,
        assign_with_copy_calls: usize,
        destroy_calls: usize,
        selection_values_constructed: usize,
        selection_values_read: usize,
        returned_destination_checks: usize,
        byte_comparisons: usize,
    }
    #[derive(Default)]
    struct Proof {
        counts: RefCell<Counts>,
        independent_allocations: Cell<bool>,
        source_destroyed_before_copy: Cell<bool>,
        verified: Cell<bool>,
    }
    impl Drop for Proof {
        fn drop(&mut self) {
            // Declared before the owners: unwinding destroys/deallocates them
            // first, then emits actual partial counts with verified=false.
            let c = self.counts.borrow();
            println!("Selection value result: phase=value_lifecycle");
            println!(
                "Selection value result: lifecycle_verified={}",
                self.verified.get()
            );
            for (name, count) in [
                ("allocations", c.allocations),
                ("deallocations", c.deallocations),
                ("initializer_calls", c.initializer_calls),
                ("initializer_wrapper_calls", c.initializer_calls),
                ("initialize_with_copy_calls", c.initialize_with_copy_calls),
                ("assign_with_copy_calls", c.assign_with_copy_calls),
                ("destroy_calls", c.destroy_calls),
                (
                    "value_witness_calls",
                    c.initialize_with_copy_calls + c.assign_with_copy_calls + c.destroy_calls,
                ),
                (
                    "selection_values_constructed",
                    c.selection_values_constructed,
                ),
                ("selection_values_read", c.selection_values_read),
                ("returned_destination_checks", c.returned_destination_checks),
                ("byte_comparisons", c.byte_comparisons),
            ] {
                println!("Selection value result: {name}={count}");
            }
            println!(
                "Selection value result: independent_allocations={}",
                self.independent_allocations.get()
            );
            println!(
                "Selection value result: source_destroyed_before_copy={}",
                self.source_destroyed_before_copy.get()
            );
            for field in [
                "binding_values_constructed",
                "binding_initializer_calls",
                "binding_getter_calls",
                "binding_setter_calls",
                "closure_calls",
                "authorization_requests",
                "private_api_calls",
                "private_api_reads",
            ] {
                println!("Selection value result: {field}=0");
            }
            for field in [
                "mutable_binding_verified",
                "mutable_values_verified",
                "picker_verified",
                "grant_verified",
                "monitoring_verified",
                "shielding_verified",
                "selection_contents_output",
            ] {
                println!("Selection value result: {field}=false");
            }
        }
    }
    struct Value<'a> {
        pointer: NonNull<u8>,
        initialized: bool,
        witnesses: &'a Witnesses,
        proof: &'a Proof,
    }
    impl<'a> Value<'a> {
        fn allocate(witnesses: &'a Witnesses, proof: &'a Proof) -> Self {
            // Entry_4 and the observed copy/assign witnesses leave padding alone.
            // Zero it before any foreign call so full-size comparisons read only
            // initialized bytes. Never interpret tokens or expose value contents.
            let pointer = NonNull::new(unsafe { alloc_zeroed(witnesses.layout) })
                .expect("selection allocation failed");
            proof.counts.borrow_mut().allocations += 1;
            Self {
                pointer,
                initialized: false,
                witnesses,
                proof,
            }
        }
        fn initialize_default(&mut self, entry: *const c_void) {
            assert!(!self.initialized, "selection already initialized");
            unsafe { rust_selection_default_init(self.pointer.as_ptr().cast(), entry) };
            self.initialized = true;
            let mut c = self.proof.counts.borrow_mut();
            c.initializer_calls += 1;
            c.selection_values_constructed += 1;
        }
        fn copy_from(&mut self, source: &Self, initialize: bool) {
            assert!(source.initialized, "copy source is not initialized");
            assert!(
                self.initialized != initialize,
                "invalid copy destination lifetime"
            );
            assert!(self.pointer != source.pointer, "copy allocations alias");
            let witness = if initialize {
                self.witnesses.initialize
            } else {
                self.witnesses.assign
            };
            let destination = self.pointer.as_ptr().cast();
            let returned = unsafe {
                witness(
                    destination,
                    source.pointer.as_ptr().cast(),
                    self.witnesses.metadata,
                )
            };
            // Mark the successful initialization before assertions can unwind.
            self.initialized = true;
            {
                let mut c = self.proof.counts.borrow_mut();
                if initialize {
                    c.initialize_with_copy_calls += 1;
                    c.selection_values_constructed += 1;
                } else {
                    c.assign_with_copy_calls += 1;
                }
            }
            assert!(
                returned == destination,
                "copy witness did not return its destination"
            );
            self.proof.counts.borrow_mut().returned_destination_checks += 1;
        }
        fn bytes(&self) -> &[u8] {
            assert!(self.initialized, "cannot read a destroyed selection");
            self.proof.counts.borrow_mut().selection_values_read += 1;
            unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.witnesses.size) }
        }
        fn compare(&self, other: &Self) {
            // Boolean assertion only: assert_eq! would print selection contents.
            assert!(
                self.bytes() == other.bytes(),
                "initialized selection bytes differ"
            );
            self.proof.counts.borrow_mut().byte_comparisons += 1;
        }
        fn destroy(&mut self) {
            if self.initialized {
                self.initialized = false;
                unsafe {
                    (self.witnesses.destroy)(self.pointer.as_ptr().cast(), self.witnesses.metadata)
                };
                self.proof.counts.borrow_mut().destroy_calls += 1;
            }
        }
    }
    impl Drop for Value<'_> {
        fn drop(&mut self) {
            self.destroy();
            unsafe { dealloc(self.pointer.as_ptr(), self.witnesses.layout) };
            self.proof.counts.borrow_mut().deallocations += 1;
        }
    }
    pub(super) fn inspect(metadata: *const c_void) {
        let proof = Proof::default();
        let page = unsafe { getpagesize() };
        assert!(page > 0, "invalid Darwin page size");
        let page = page as usize;
        let table = unsafe {
            read(
                metadata.cast::<*const ValueWitnessTable>().wrapping_sub(1),
                page,
            )
        };
        assert!(!table.is_null(), "null selection witnesses");
        let size = unsafe { read(&raw const (*table).size, page) };
        let stride = unsafe { read(&raw const (*table).stride, page) };
        let flags = unsafe { read(&raw const (*table).flags, page) };
        let alignment = (flags as usize & 0xff) + 1;
        assert!(flags & 0x0040_0000 == 0, "incomplete selection layout");
        assert!(
            (size, stride, alignment) == (56, 56, 8),
            "SDK selection layout changed"
        );
        let entry = |slot: usize| {
            let pointer = unsafe {
                read(
                    (&raw const (*table).functions)
                        .cast::<*const c_void>()
                        .add(slot),
                    page,
                )
            };
            assert!(
                !pointer.is_null() && pointer.addr() % 4 == 0,
                "invalid selection witness entry"
            );
            pointer
        };
        let witnesses = Witnesses {
            metadata,
            destroy: unsafe { std::mem::transmute::<*const c_void, Destroy>(entry(1)) },
            initialize: unsafe { std::mem::transmute::<*const c_void, Copy>(entry(2)) },
            assign: unsafe { std::mem::transmute::<*const c_void, Copy>(entry(3)) },
            layout: Layout::from_size_align(stride, alignment)
                .expect("invalid selection allocation layout"),
            size,
        };
        let initializer = resolve(env!("SELECTION_INIT_SYMBOL"));
        let mut first = Value::allocate(&witnesses, &proof);
        let mut second = Value::allocate(&witnesses, &proof);
        let mut third = Value::allocate(&witnesses, &proof);
        for (a, b) in [(&first, &second), (&first, &third), (&second, &third)] {
            let low = a.pointer.addr().get().min(b.pointer.addr().get());
            let high = a.pointer.addr().get().max(b.pointer.addr().get());
            assert!(high - low >= stride, "selection allocations overlap");
            assert!(
                low % alignment == 0 && high % alignment == 0,
                "selection allocation misaligned"
            );
        }
        proof.independent_allocations.set(true);
        first.initialize_default(initializer);
        second.copy_from(&first, true);
        first.compare(&second);
        first.destroy();
        assert!(
            !first.initialized && second.initialized,
            "source destruction invalidated the copy"
        );
        third.copy_from(&second, true);
        proof.source_destroyed_before_copy.set(true);
        second.compare(&third);
        third.copy_from(&second, false);
        second.compare(&third);
        drop(first);
        drop(second);
        drop(third);
        {
            let c = proof.counts.borrow();
            assert!(
                (
                    c.allocations,
                    c.deallocations,
                    c.initializer_calls,
                    c.initialize_with_copy_calls,
                    c.assign_with_copy_calls,
                    c.destroy_calls
                ) == (3, 3, 1, 2, 1, 3),
                "selection lifecycle counts mismatch"
            );
            assert!(
                (
                    c.selection_values_constructed,
                    c.selection_values_read,
                    c.returned_destination_checks,
                    c.byte_comparisons
                ) == (3, 6, 3, 3),
                "selection lifecycle proof incomplete"
            );
        }
        proof.verified.set(true);
    }
}

// Finite, request-free mutable value experiment. Evidence: Selection VWT
// run38077219284, category entry run38082809973, and null-context boundary
// research050528 (Swift 5.10 SILFunctionType/CallingConvention/HeapObject).
#[cfg(all(mutable_binding_probe, selection_layout_probe))]
mod mutable_binding {
    use super::MetadataResponse;
    use super::selection_layout::{ValueWitnessTable, read, resolve};
    use std::alloc::{Layout, alloc_zeroed, dealloc};
    use std::cell::RefCell;
    use std::ffi::{c_char, c_void};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    // Every C/Swift boundary saves x20 and LR and has a 16-byte C stack/CFI.
    // Category: C(dst, Bool, entry) -> Swift x8=dst, x0=Bool, no self/metatype.
    // Binding init: C(dst, get, set, metadata, entry) -> x8, x0/NULL,
    // x2/NULL, x4=Value metadata (not an explicit metatype argument).
    // Public get/set: x20 is Binding self; x0(get)/x1(set) is Binding
    // metadata, from which the SDK projects Value at +16.
    // Closure x20 is NULL context.
    // Closure metadata registers are deliberately unused: the stable state
    // supplies the already validated metadata, so no extra-argument assumption.
    core::arch::global_asm!(
        ".text",
        ".p2align 2",
        ".globl _rust_mutable_category_init",
        "_rust_mutable_category_init:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x8, x0",
        "mov x0, x1",
        "blr x2",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
        ".p2align 2",
        ".globl _rust_mutable_binding_init",
        "_rust_mutable_binding_init:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x8, x0",
        "mov x0, x1",
        "mov x1, xzr",
        "mov x9, x4",
        "mov x4, x3",
        "mov x3, xzr",
        "blr x9",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
        ".p2align 2",
        ".globl _rust_mutable_binding_get",
        "_rust_mutable_binding_get:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x20, x0",
        "mov x8, x1",
        "mov x0, x2",
        "blr x3",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
        ".p2align 2",
        ".globl _rust_mutable_binding_set",
        "_rust_mutable_binding_set:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x20, x0",
        "mov x0, x1",
        "mov x1, x2",
        "blr x3",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
        ".p2align 2",
        ".globl _rust_mutable_get_closure",
        "_rust_mutable_get_closure:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x0, x8",
        "mov x1, x20",
        "bl _rust_mutable_get_helper",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
        ".p2align 2",
        ".globl _rust_mutable_set_closure",
        "_rust_mutable_set_closure:",
        ".cfi_startproc",
        "stp x20, x30, [sp, #-16]!",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset 20, -16",
        ".cfi_offset 30, -8",
        "mov x1, x20",
        "bl _rust_mutable_set_helper",
        "ldp x20, x30, [sp], #16",
        ".cfi_restore 20",
        ".cfi_restore 30",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
    );
    unsafe extern "C" {
        fn rust_mutable_category_init(dst: *mut c_void, flag: usize, entry: *const c_void);
        fn rust_mutable_binding_init(
            dst: *mut c_void,
            get: *const c_void,
            set: *const c_void,
            metadata: *const c_void,
            entry: *const c_void,
        );
        fn rust_mutable_binding_get(
            value: *mut c_void,
            dst: *mut c_void,
            metadata: *const c_void,
            entry: *const c_void,
        );
        fn rust_mutable_binding_set(
            value: *mut c_void,
            source: *mut c_void,
            metadata: *const c_void,
            entry: *const c_void,
        );
        fn rust_mutable_get_closure();
        fn rust_mutable_set_closure();
        fn getpagesize() -> i32;
        fn pthread_main_np() -> i32;
        fn dladdr(address: *const c_void, information: *mut DlInfo) -> i32;
    }
    #[repr(C)]
    #[derive(Default)]
    struct DlInfo {
        filename: *const c_char,
        base: *const c_void,
        symbol: *const c_char,
        symbol_address: *const c_void,
    }
    type Destroy = unsafe extern "Swift" fn(*mut c_void, *const c_void);
    type Copy = unsafe extern "Swift" fn(*mut c_void, *mut c_void, *const c_void) -> *mut c_void;
    struct Witnesses {
        metadata: *const c_void,
        destroy: Destroy,
        initialize: Copy,
        assign: Copy,
        layout: Layout,
        size: usize,
    }
    impl Witnesses {
        fn load(metadata: *const c_void, expected: usize, page: usize) -> Self {
            assert!(
                unsafe { read(metadata.cast::<usize>(), page) } == 0x200,
                "mutable value metadata must be a struct"
            );
            let table = unsafe {
                read(
                    metadata.cast::<*const ValueWitnessTable>().wrapping_sub(1),
                    page,
                )
            };
            assert!(!table.is_null(), "mutable value witnesses missing");
            let size = unsafe { read(&raw const (*table).size, page) };
            let stride = unsafe { read(&raw const (*table).stride, page) };
            let flags = unsafe { read(&raw const (*table).flags, page) };
            let alignment = (flags as usize & 0xff) + 1;
            assert!(
                flags & 0x0040_0000 == 0 && (size, stride, alignment) == (expected, expected, 8),
                "mutable value layout changed"
            );
            let entry = |slot| {
                let p = unsafe {
                    read(
                        (&raw const (*table).functions)
                            .cast::<*const c_void>()
                            .add(slot),
                        page,
                    )
                };
                assert!(
                    !p.is_null() && p.addr() % 4 == 0,
                    "invalid mutable value witness"
                );
                p
            };
            Self {
                metadata,
                size,
                layout: Layout::from_size_align(stride, alignment).unwrap(),
                destroy: unsafe { std::mem::transmute::<*const c_void, Destroy>(entry(1)) },
                initialize: unsafe { std::mem::transmute::<*const c_void, Copy>(entry(2)) },
                assign: unsafe { std::mem::transmute::<*const c_void, Copy>(entry(3)) },
            }
        }
    }
    #[derive(Clone, Copy)]
    enum Count {
        Alloc,
        Dealloc,
        Category,
        BindingInit,
        PublicGet,
        PublicSet,
        ClosureGet,
        ClosureSet,
        InitCopy,
        AssignCopy,
        Destroy,
        BindingCopy,
        BindingDestroy,
        Taken,
        Returned,
        Comparisons,
        Active,
    }
    const COUNT_NAMES: [&str; 17] = [
        "allocations",
        "deallocations",
        "category_initializer_calls",
        "binding_initializer_calls",
        "binding_getter_calls",
        "binding_setter_calls",
        "getter_closure_calls",
        "setter_closure_calls",
        "initialize_with_copy_calls",
        "assign_with_copy_calls",
        "destroy_calls",
        "binding_initialize_with_copy_calls",
        "binding_destroy_calls",
        "setter_inputs_transferred",
        "returned_destination_checks",
        "byte_comparisons",
        "callbacks_active",
    ];
    struct Proof {
        counts: [AtomicUsize; 17],
        verified: AtomicBool,
        comparisons: [AtomicBool; 5],
        started: Instant,
    }
    impl Proof {
        fn new() -> Self {
            Self {
                counts: std::array::from_fn(|_| AtomicUsize::new(0)),
                verified: AtomicBool::new(false),
                comparisons: std::array::from_fn(|_| AtomicBool::new(false)),
                started: Instant::now(),
            }
        }
        fn inc(&self, c: Count) {
            self.counts[c as usize].fetch_add(1, Ordering::SeqCst);
        }
        fn n(&self, c: Count) -> usize {
            self.counts[c as usize].load(Ordering::SeqCst)
        }
        fn check(&self) {
            assert!(
                unsafe { pthread_main_np() } == 1,
                "mutable binding requires the main thread"
            );
            assert!(
                self.started.elapsed() < Duration::from_secs(30),
                "mutable binding deadline exceeded"
            );
        }
        fn emit(&self) {
            println!("Mutable binding result: phase=mutable_value");
            println!(
                "Mutable binding result: mutable_binding_verified={}",
                self.verified.load(Ordering::SeqCst)
            );
            for (i, name) in COUNT_NAMES.iter().enumerate() {
                println!(
                    "Mutable binding result: {name}={}",
                    self.counts[i].load(Ordering::SeqCst)
                );
            }
            for (i, name) in [
                "category_values_differ",
                "initial_get_equals_false",
                "changed_get_equals_true",
                "changed_get_differs_false",
                "restored_get_equals_false",
            ]
            .iter()
            .enumerate()
            {
                println!(
                    "Mutable binding result: {name}={}",
                    self.comparisons[i].load(Ordering::SeqCst)
                );
            }
            for name in ["null_contexts", "binding_full_vwt", "main_thread_required"] {
                println!("Mutable binding result: {name}=true");
            }
            for name in [
                "picker_verified",
                "tokens_verified",
                "grant_verified",
                "monitoring_verified",
                "shielding_verified",
                "selection_contents_output",
                "selection_addresses_output",
            ] {
                println!("Mutable binding result: {name}=false");
            }
            for name in [
                "authorization_requests",
                "private_api_calls",
                "private_api_reads",
            ] {
                println!("Mutable binding result: {name}=0");
            }
        }
    }
    impl Drop for Proof {
        fn drop(&mut self) {
            self.emit();
        }
    }
    // A separate watchdog terminates even if a foreign synchronous call stalls.
    // It reads atomic counters only, never the state or Selection memory.
    struct Deadline {
        done: Arc<(Mutex<bool>, Condvar)>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Deadline {
        fn start(proof: Arc<Proof>) -> Self {
            let done = Arc::new((Mutex::new(false), Condvar::new()));
            let signal = done.clone();
            let thread = std::thread::spawn(move || {
                let (lock, wake) = &*signal;
                let finished = wake
                    .wait_timeout_while(
                        lock.lock().unwrap(),
                        Duration::from_secs(30).saturating_sub(proof.started.elapsed()),
                        |done| !*done,
                    )
                    .unwrap();
                if !*finished.0 {
                    proof.verified.store(false, Ordering::SeqCst);
                    proof.emit();
                    eprintln!("Mutable binding foreign-call deadline exceeded");
                    std::process::exit(1);
                }
            });
            Self {
                done,
                thread: Some(thread),
            }
        }
    }
    impl Drop for Deadline {
        fn drop(&mut self) {
            *self.done.0.lock().unwrap() = true;
            self.done.1.notify_all();
            if let Some(thread) = self.thread.take() {
                thread.join().expect("mutable deadline thread failed");
            }
        }
    }
    struct Value<'a> {
        pointer: NonNull<u8>,
        initialized: bool,
        taken: bool,
        witnesses: &'a Witnesses,
        proof: &'a Proof,
        binding: bool,
    }
    impl<'a> Value<'a> {
        fn allocate(witnesses: &'a Witnesses, proof: &'a Proof, binding: bool) -> Self {
            let pointer = NonNull::new(unsafe { alloc_zeroed(witnesses.layout) })
                .expect("mutable allocation failed");
            proof.inc(Count::Alloc);
            Self {
                pointer,
                initialized: false,
                taken: false,
                witnesses,
                proof,
                binding,
            }
        }
        fn ptr(&self) -> *mut c_void {
            self.pointer.as_ptr().cast()
        }
        fn category(&mut self, flag: bool, entry: *const c_void) {
            self.proof.check();
            assert!(!self.initialized && !self.taken && !self.binding);
            self.proof.inc(Count::Category);
            unsafe { rust_mutable_category_init(self.ptr(), usize::from(flag), entry) };
            self.initialized = true;
        }
        fn copy(&mut self, source: &Self) {
            self.proof.check();
            assert!(!self.initialized && !self.taken && source.initialized);
            assert!(
                self.witnesses.metadata == source.witnesses.metadata && self.ptr() != source.ptr()
            );
            self.proof.inc(if self.binding {
                Count::BindingCopy
            } else {
                Count::InitCopy
            });
            let result = unsafe {
                (self.witnesses.initialize)(self.ptr(), source.ptr(), self.witnesses.metadata)
            };
            self.initialized = true;
            assert!(
                result == self.ptr(),
                "mutable copy returned another destination"
            );
            self.proof.inc(Count::Returned);
        }
        fn compare(&self, other: &Self, slot: usize, equal: bool) {
            self.proof.check();
            assert!(self.initialized && other.initialized && !self.binding && !other.binding);
            assert!(
                self.witnesses.metadata == other.witnesses.metadata && self.ptr() != other.ptr()
            );
            let a =
                unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.witnesses.size) };
            let b =
                unsafe { std::slice::from_raw_parts(other.pointer.as_ptr(), other.witnesses.size) };
            self.proof.inc(Count::Comparisons);
            assert!((a == b) == equal, "opaque mutable comparison failed");
            self.proof.comparisons[slot].store(true, Ordering::SeqCst);
        }
        fn get(&self, output: &mut Self, metadata: *const c_void, entry: *const c_void) {
            self.proof.check();
            assert!(self.binding && self.initialized && !output.initialized && !output.taken);
            assert!(output.witnesses.metadata == metadata && !output.binding);
            let before = self.proof.n(Count::ClosureGet);
            self.proof.inc(Count::PublicGet);
            // The generic accessor receives Binding<Value> metadata, then
            // projects its Value argument at +16. The initializer instead
            // receives the direct Value metadata argument.
            unsafe {
                rust_mutable_binding_get(self.ptr(), output.ptr(), self.witnesses.metadata, entry)
            };
            output.initialized = true; // only after the public getter returns
            assert!(
                self.proof.n(Count::ClosureGet) > before,
                "public getter did not invoke the mutable closure"
            );
        }
        fn set(&self, input: &mut Self, metadata: *const c_void, entry: *const c_void) {
            self.proof.check();
            assert!(self.binding && self.initialized && input.initialized && !input.taken);
            assert!(input.witnesses.metadata == metadata && !input.binding);
            // The public setter owns this dedicated copy. Its closure borrows it;
            // the outer SDK setter destroys it. Retain just the allocation until
            // the call returns; Rust must never destroy this consumed value.
            input.initialized = false;
            input.taken = true;
            self.proof.inc(Count::Taken);
            self.proof.inc(Count::PublicSet);
            unsafe {
                rust_mutable_binding_set(self.ptr(), input.ptr(), self.witnesses.metadata, entry)
            };
        }
    }
    impl Drop for Value<'_> {
        fn drop(&mut self) {
            if self.initialized {
                self.initialized = false;
                self.proof.inc(if self.binding {
                    Count::BindingDestroy
                } else {
                    Count::Destroy
                });
                unsafe { (self.witnesses.destroy)(self.ptr(), self.witnesses.metadata) };
            }
            unsafe { dealloc(self.pointer.as_ptr(), self.witnesses.layout) };
            self.proof.inc(Count::Dealloc);
        }
    }
    struct State<'a> {
        value: RefCell<Value<'a>>,
        proof: &'a Proof,
    }
    static STATE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
    // Failure diagnostics may run on a rejected foreign callback thread. Keep
    // only atomic counters here, independently of the main-thread state lifetime.
    static DIAGNOSTICS: Mutex<Option<Arc<Proof>>> = Mutex::new(None);
    struct Installed<'a> {
        state: Box<State<'a>>,
    }
    impl<'a> Installed<'a> {
        fn install(value: Value<'a>, proof: &'a Arc<Proof>) -> Self {
            assert!(value.initialized && !value.binding);
            proof.check();
            let mut state = Box::new(State {
                value: RefCell::new(value),
                proof,
            });
            let pointer = (&raw mut *state).cast::<c_void>();
            assert!(
                STATE
                    .compare_exchange(
                        std::ptr::null_mut(),
                        pointer,
                        Ordering::SeqCst,
                        Ordering::SeqCst
                    )
                    .is_ok(),
                "mutable state already installed"
            );
            let owner = Self { state };
            let mut diagnostics = DIAGNOSTICS.lock().unwrap();
            assert!(
                diagnostics.is_none(),
                "mutable diagnostics already installed"
            );
            *diagnostics = Some(proof.clone());
            owner
        }
    }
    impl Drop for Installed<'_> {
        fn drop(&mut self) {
            // Declared before all Binding owners: their full VWT destroy completes
            // before this guard. NULL contexts cannot escape into an owned capture.
            assert!(
                self.state.proof.n(Count::Active) == 0,
                "mutable callbacks still active"
            );
            let p = (&raw mut *self.state).cast::<c_void>();
            assert!(
                STATE
                    .compare_exchange(p, std::ptr::null_mut(), Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok(),
                "mutable state ownership changed"
            );
            DIAGNOSTICS.lock().unwrap().take();
            // Box drops initialized state through Selection VWT after unregistering.
        }
    }
    struct Callback<'a>(&'a Proof);
    impl Drop for Callback<'_> {
        fn drop(&mut self) {
            self.0.counts[Count::Active as usize].fetch_sub(1, Ordering::SeqCst);
        }
    }
    fn callback(pointer: *mut c_void, context: *mut c_void, getter: bool) {
        // C helpers cannot unwind through Swift. Failed validation emits actual
        // partial counters and exits; no attempt to destroy an unknown foreign
        // partial result. Successful calls participate in ordinary RAII cleanup.
        let diagnostics = DIAGNOSTICS.lock().unwrap().clone();
        if let Some(proof) = &diagnostics {
            proof.inc(if getter {
                Count::ClosureGet
            } else {
                Count::ClosureSet
            });
        }
        let outcome = std::panic::catch_unwind(|| {
            assert!(
                unsafe { pthread_main_np() } == 1,
                "foreign mutable callback thread"
            );
            let p = STATE.load(Ordering::SeqCst);
            assert!(!p.is_null(), "mutable callback without initialized state");
            let state = unsafe { &*p.cast::<State<'_>>() };
            state.proof.check();
            assert!(context.is_null(), "mutable callback context is not NULL");
            assert!(
                state.proof.n(Count::Active) == 0,
                "mutable callback reentry"
            );
            state.proof.inc(Count::Active);
            let _active = Callback(state.proof);
            let value = state.value.borrow();
            assert!(
                value.initialized
                    && !pointer.is_null()
                    && pointer.addr() % value.witnesses.layout.align() == 0,
                "invalid mutable callback value"
            );
            let distance = pointer.addr().abs_diff(value.ptr().addr());
            assert!(
                distance >= value.witnesses.layout.size(),
                "mutable callback overlaps stable state"
            );
            let result = if getter {
                state.proof.inc(Count::InitCopy);
                unsafe {
                    (value.witnesses.initialize)(pointer, value.ptr(), value.witnesses.metadata)
                }
            } else {
                state.proof.inc(Count::AssignCopy);
                unsafe { (value.witnesses.assign)(value.ptr(), pointer, value.witnesses.metadata) }
            };
            assert!(
                result == if getter { pointer } else { value.ptr() },
                "mutable closure witness destination changed"
            );
            state.proof.inc(Count::Returned);
        });
        if outcome.is_err() {
            if let Some(proof) = diagnostics {
                proof.verified.store(false, Ordering::SeqCst);
                proof.emit();
            }
            eprintln!("Mutable binding callback validation failed");
            std::process::exit(1);
        }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn rust_mutable_get_helper(dst: *mut c_void, context: *mut c_void) {
        callback(dst, context, true);
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn rust_mutable_set_helper(source: *mut c_void, context: *mut c_void) {
        callback(source, context, false);
    }

    fn word(entry: *const c_void, offset: usize, page: usize) -> u32 {
        unsafe { read(entry.cast::<u8>().wrapping_add(offset).cast::<u32>(), page) }
    }
    fn branch(entry: *const c_void, offset: usize, call: bool, page: usize) -> *const c_void {
        let instruction = word(entry, offset, page);
        assert!(
            instruction & 0xfc00_0000 == if call { 0x9400_0000 } else { 0x1400_0000 },
            "SDK branch shape changed"
        );
        let relative = ((instruction << 6) as i32 >> 6) as isize * 4;
        let displacement = (offset as isize).checked_add(relative).unwrap();
        let address = entry
            .addr()
            .checked_add_signed(displacement)
            .expect("SDK branch address overflow");
        let target = entry
            .cast::<u8>()
            .wrapping_offset(displacement)
            .cast::<c_void>();
        assert!(address == target.addr() && address % 4 == 0);
        let mut a = DlInfo::default();
        let mut b = DlInfo::default();
        assert!(
            unsafe { dladdr(entry, &mut a) } != 0
                && unsafe { dladdr(target, &mut b) } != 0
                && !a.base.is_null()
                && a.base == b.base,
            "SDK branch changed image"
        );
        target
    }
    fn verify_entries(
        category: *const c_void,
        init: *const c_void,
        get: *const c_void,
        set: *const c_void,
        page: usize,
    ) {
        // Only the 28 actual SDK bytes up to the first RET are interpreted.
        // ADRP relocation bits are variable; reconstructed addresses are never
        // treated as actual SDK addresses or followed for category constants.
        for (offset, expected) in [
            (0, 0x3900_0100),
            (12, 0xa900_a509),
            (16, 0xa901_a509),
            (20, 0xa902_a509),
            (24, 0xd65f_03c0),
        ] {
            assert!(
                word(category, offset, page) == expected,
                "category initializer prefix changed"
            );
        }
        assert!(
            word(category, 4, page) & 0x9f00_001f == 0x9000_0009
                && word(category, 8, page) & 0xffc0_03ff == 0xf940_0129,
            "category constant load shape changed"
        );
        for (offset, expected) in [
            (0, 0xa9ba_6ffc),
            (4, 0xaa1e_03e5),
            (12, 0xaa05_03fe),
            (16, 0xa905_7bfd),
            (20, 0x9101_43fd),
            (24, 0xd100_c3ff),
            (28, 0xaa04_03fa),
            (32, 0xaa03_03f3),
            (36, 0xaa02_03f4),
        ] {
            assert!(
                word(init, offset, page) == expected,
                "Binding constructor prefix changed"
            );
        }
        let _save = branch(init, 8, true, page);
        let helper = branch(init, 40, true, page);
        for (offset, expected) in [(0, 0xaa01_03f5), (4, 0xaa00_03fb), (8, 0xf818_83a8)] {
            assert!(
                word(helper, offset, page) == expected,
                "Binding constructor helper changed"
            );
        }
        let getter = branch(get, 0, false, page);
        for (offset, expected) in [
            (0, 0xa9bd_57f6),
            (4, 0xa901_4ff4),
            (8, 0xa902_7bfd),
            (12, 0x9100_83fd),
            (16, 0xaa14_03f5),
            (20, 0xaa00_03f6),
            (24, 0xaa08_03f3),
            (104, 0xb980_22c8),
            (112, 0xf940_0ac2),
        ] {
            assert!(
                word(getter, offset, page) == expected,
                "Binding getter prefix changed"
            );
        }
        for (offset, expected) in [
            (0, 0xa9bd_57f6),
            (4, 0xa901_4ff4),
            (8, 0xa902_7bfd),
            (12, 0x9100_83fd),
            (16, 0xaa01_03f3),
            (20, 0xaa00_03f5),
            (24, 0xa940_2281),
            (28, 0xf940_0109),
            (32, 0xf940_4129),
            (36, 0xaa08_03f4),
            (40, 0xd63f_0120),
            (44, 0xf940_0a61),
            (48, 0xf85f_8028),
            (52, 0xf940_0502),
            (56, 0xaa15_03e0),
            (60, 0xa942_7bfd),
            (64, 0xa941_4ff4),
            (68, 0xa8c3_57f6),
            (72, 0xd61f_0040),
        ] {
            assert!(
                word(set, offset, page) == expected,
                "Binding setter ownership prefix changed"
            );
        }
        println!(
            "Mutable binding entry guards: category_first28=true; binding_register_prefixes=true; setter_outer_destroy=true; reconstructed_addresses_used=false"
        );
    }
    pub(super) fn inspect(metadata: *const c_void) {
        let proof = Arc::new(Proof::new());
        let _deadline = Deadline::start(proof.clone());
        proof.check();
        let page = unsafe { getpagesize() };
        assert!(page > 0);
        let page = page as usize;
        let selection = Witnesses::load(metadata, 56, page);
        let accessor: unsafe extern "Swift" fn(usize, *const c_void) -> MetadataResponse =
            unsafe { std::mem::transmute(resolve(env!("BINDING_METADATA_SYMBOL"))) };
        let response = unsafe { accessor(0, metadata) };
        assert!(
            response.state == 0 && !response.value.is_null(),
            "Binding metadata incomplete"
        );
        let binding_type = Witnesses::load(response.value, 72, page);
        // Swift5.10 StructMetadata has kind + descriptor before its generic
        // arguments. Actual get body uses [x0+16], and set uses [x1+16],
        // to project Value metadata. Validate that specialization before calls.
        assert!(
            unsafe { read(response.value.cast::<*const c_void>().wrapping_add(2), page) }
                == metadata,
            "Binding generic Value metadata differs"
        );
        let category = resolve(env!("SELECTION_CATEGORY_INIT_SYMBOL"));
        let init = resolve(env!("BINDING_INIT_SYMBOL"));
        let get = resolve(env!("BINDING_GET_SYMBOL"));
        let set = resolve(env!("BINDING_SET_SYMBOL"));
        verify_entries(category, init, get, set, page);
        let mut initial = Value::allocate(&selection, &proof, false);
        initial.category(false, category);
        let mut changed = Value::allocate(&selection, &proof, false);
        changed.category(true, category);
        initial.compare(&changed, 0, false);
        let mut stable = Value::allocate(&selection, &proof, false);
        stable.copy(&initial);
        // Stable boxed Rust state is installed BEFORE the actual constructor,
        // which may synchronously invoke the getter. No Swift heap context.
        let state = Installed::install(stable, &proof);
        let mut binding = Value::allocate(&binding_type, &proof, true);
        proof.inc(Count::BindingInit);
        unsafe {
            rust_mutable_binding_init(
                binding.ptr(),
                rust_mutable_get_closure as *const c_void,
                rust_mutable_set_closure as *const c_void,
                metadata,
                init,
            )
        };
        binding.initialized = true;
        // Copy/destroy the ENTIRE initialized Binding using its own VWT, then
        // continue with the copy after destroying the original allocation.
        let mut binding_copy = Value::allocate(&binding_type, &proof, true);
        binding_copy.copy(&binding);
        drop(binding);
        let mut first = Value::allocate(&selection, &proof, false);
        binding_copy.get(&mut first, metadata, get);
        first.compare(&initial, 1, true);
        let mut input = Value::allocate(&selection, &proof, false);
        input.copy(&changed);
        let before = proof.n(Count::ClosureSet);
        binding_copy.set(&mut input, metadata, set);
        assert!(
            proof.n(Count::ClosureSet) == before + 1,
            "public setter did not call the mutable closure once"
        );
        drop(input); // consumed allocation only, no Rust destroy
        let mut after = Value::allocate(&selection, &proof, false);
        binding_copy.get(&mut after, metadata, get);
        after.compare(&changed, 2, true);
        after.compare(&initial, 3, false);
        let mut restore = Value::allocate(&selection, &proof, false);
        restore.copy(&initial);
        let before = proof.n(Count::ClosureSet);
        binding_copy.set(&mut restore, metadata, set);
        assert!(
            proof.n(Count::ClosureSet) == before + 1,
            "restore did not call the mutable closure once"
        );
        drop(restore);
        let mut last = Value::allocate(&selection, &proof, false);
        binding_copy.get(&mut last, metadata, get);
        last.compare(&initial, 4, true);
        drop(first);
        drop(after);
        drop(last);
        drop(binding_copy); // no remaining owned Binding or callable closure
        assert!(proof.n(Count::Active) == 0);
        drop(state);
        drop(changed);
        drop(initial);
        proof.check();
        assert!(
            proof.n(Count::Alloc) == proof.n(Count::Dealloc)
                && proof.n(Count::Category) == 2
                && proof.n(Count::BindingInit) == 1
                && proof.n(Count::BindingCopy) == 1
                && proof.n(Count::BindingDestroy) == 2
                && proof.n(Count::PublicGet) == 3
                && proof.n(Count::PublicSet) == 2
                && proof.n(Count::ClosureGet) >= 3
                && proof.n(Count::ClosureSet) == 2
                && proof.n(Count::AssignCopy) == proof.n(Count::ClosureSet)
                && proof.n(Count::InitCopy) == 3 + proof.n(Count::ClosureGet)
                && proof.n(Count::Returned)
                    == proof.n(Count::InitCopy)
                        + proof.n(Count::AssignCopy)
                        + proof.n(Count::BindingCopy)
                && proof.n(Count::Taken) == 2
                && proof.n(Count::Destroy) == 6
                && proof.n(Count::Comparisons) == 5
                && proof.n(Count::Active) == 0,
            "mutable lifecycle counts inconsistent"
        );
        proof.verified.store(true, Ordering::SeqCst);
    }
}

#[cfg(runtime_probe)]
#[cfg_attr(any(uikit_host, authorization_status_entry_probe), allow(dead_code))]
mod async_inspection {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    // Swift 5.10 include/swift/ABI/Executor.h: AsyncFunctionPointer.
    // This probe is limited to non-pointer-authenticated arm64 Simulator code.
    #[repr(C)]
    struct AsyncFunctionPointer {
        entry_offset: i32,
        context_size: u32,
    }

    #[repr(C)]
    pub(super) struct TaskAndContext {
        pub(super) task: *mut c_void,
        pub(super) context: *mut c_void,
    }

    #[link(name = "FamilyControls", kind = "framework")]
    unsafe extern "C" {
        #[link_name = env!("AUTH_ASYNC_SYMBOL")]
        static AUTHORIZATION_POINTER: AsyncFunctionPointer;
    }

    #[link(name = "swift_Concurrency")]
    unsafe extern "Swift" {
        pub(super) fn swift_task_create_common(
            flags: usize,
            options: *const c_void,
            result_type: *const c_void,
            entry: *const c_void,
            closure: *const c_void,
            context_size: usize,
        ) -> TaskAndContext;
    }

    #[link(name = "swiftCore")]
    unsafe extern "Swift" {
        fn swift_release(object: *mut c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }

    unsafe extern "C" {
        fn rust_async_smoke_entry();
    }

    static ENTERED: AtomicBool = AtomicBool::new(false);

    #[unsafe(no_mangle)]
    extern "C" fn rust_async_smoke_report() {
        ENTERED.store(true, Ordering::Release);
    }

    // Runtime non_future_adapter passes the initial context in x22, and its
    // null closure in x20. AsyncContext.ResumeParent is at byte offset 8.
    // Swift 5.10 Task.cpp installs completeTaskAndRelease here. Its error
    // argument uses swiftself/x20, NOT the synchronous swifterror/x21.
    // All synchronous stack storage is removed before the async tail call.
    core::arch::global_asm!(
        ".text",
        ".p2align 2",
        ".globl _rust_async_smoke_entry",
        "_rust_async_smoke_entry:",
        "stp x29, x30, [sp, #-16]!",
        "mov x29, sp",
        "bl _rust_async_smoke_report",
        "ldp x29, x30, [sp], #16",
        "ldr x16, [x22, #8]",
        "mov x20, #0",
        "br x16",
    );

    pub fn inspect() {
        let pointer = &raw const AUTHORIZATION_POINTER;
        let descriptor = unsafe { &*pointer };
        assert_ne!(descriptor.entry_offset, 0, "null relative async entry");
        assert!(descriptor.context_size >= 16 && descriptor.context_size <= 1_048_576);
        let entry = pointer
            .cast::<u8>()
            .wrapping_offset(descriptor.entry_offset as isize);
        assert_eq!(entry.addr() % 4, 0, "unaligned arm64 async entry");
        println!(
            "Public async descriptor: offset={}, context_size={}",
            descriptor.entry_offset, descriptor.context_size
        );

        // EnqueueJob bit 12 from Swift 5.10 MetadataValues.h. No state is
        // written to InitialContext after enqueue: only runtime initializes it.
        // A null closure keeps the runtime from releasing a Rust allocation.
        let task = unsafe {
            swift_task_create_common(
                1 << 12,
                std::ptr::null(),
                std::ptr::null(),
                rust_async_smoke_entry as *const c_void,
                std::ptr::null(),
                32,
            )
        };
        assert!(!task.task.is_null() && !task.context.is_null());
        // Drop the create caller's +1; queued/running task owns a separate +1.
        unsafe { swift_release(task.task) };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ENTERED.load(Ordering::Acquire) && Instant::now() < deadline {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 1) };
        }
        assert!(
            ENTERED.load(Ordering::Acquire),
            "Swift runtime task entry timed out"
        );
        println!("Rust async task entry observed; authorization_requested=false");
        // ENTERED proves entry only. It is not a runtime teardown assertion.
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0) };
    }
}

// Prepared separately while the descriptor inspection runs. Merge into the
// existing two-file audit only after the real SDK descriptor is confirmed.
#[cfg(all(runtime_probe, auth_request))]
#[cfg_attr(selection_layout_probe, allow(dead_code))]
mod request_probe {
    use super::async_inspection::swift_task_create_common;
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[repr(C)]
    struct Descriptor {
        offset: i32,
        size: u32,
    }
    #[repr(C)]
    struct Executor {
        identity: *const c_void,
        implementation: usize,
    }
    #[repr(C)]
    struct ExecutorOption {
        flags: usize,
        parent: *const c_void,
        executor: Executor,
    }
    #[repr(C, align(16))]
    struct Root {
        parent: *mut c_void,
        resume_parent: *const c_void,
        child: *mut c_void,
        entry: *const c_void,
        object: *mut c_void,
        member: isize,
    }
    #[repr(C, align(16))]
    struct Header {
        parent: *mut Root,
        resume_parent: *const c_void,
    }
    struct Input {
        entry: usize,
        size: usize,
        object: usize,
        member: isize,
    }
    static INPUT: OnceLock<Input> = OnceLock::new();
    // 0 pending, 1 returned normally, 2 returned an error. Neither outcome is
    // represented as verified authorization status, monitoring, or shielding.
    static RESULT: AtomicU8 = AtomicU8::new(0);
    static CALLER_ROOT: AtomicUsize = AtomicUsize::new(0);

    #[link(name = "FamilyControls", kind = "framework")]
    unsafe extern "C" {
        #[link_name = env!("AUTH_ASYNC_SYMBOL")]
        static REQUEST: Descriptor;
    }
    #[link(name = "swift_Concurrency")]
    unsafe extern "Swift" {
        fn swift_task_alloc(size: usize) -> *mut c_void;
        fn swift_task_dealloc(pointer: *mut c_void);
        fn swift_task_getMainExecutor() -> Executor;
        fn swift_task_cancel(task: *mut c_void);
    }
    #[link(name = "swiftCore")]
    unsafe extern "Swift" {
        fn swift_release(object: *mut c_void);
    }
    #[link(name = "swiftCore")]
    unsafe extern "Swift" {
        // Swift 5.10 ErrorObject.h/mm: compiler runtime bridge consumes an
        // owned Error box and returns an owned NSError. This is version-pinned
        // Swift runtime SPI, not a FamilyControls private authorization API.
        fn _swift_stdlib_bridgeErrorToNSError(error: *mut c_void) -> *mut c_void;
    }
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "objc")]
    unsafe extern "C" {
        fn sel_registerName(name: *const std::ffi::c_char) -> *const c_void;
        // These three zero-argument selectors return a pointer or NSInteger:
        // all use one integer result register on this arm64 target.
        pub(super) fn objc_msgSend(object: *mut c_void, selector: *const c_void) -> usize;
        fn objc_release(object: *mut c_void);
    }
    #[cfg(not(uikit_host))]
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }
    unsafe extern "C" {
        fn rust_authorization_entry();
        fn rust_authorization_resume();
    }
    pub(super) struct TaskOwner(*mut c_void);
    // The retained task handle supports concurrent cancellation and ARC; no
    // task-context allocator or UIKit object is accessed through this handle.
    #[cfg(uikit_host)]
    unsafe impl Send for TaskOwner {}
    impl TaskOwner {
        fn cancel(&self) {
            unsafe { swift_task_cancel(self.0) };
        }
    }
    impl Drop for TaskOwner {
        fn drop(&mut self) {
            unsafe { swift_release(self.0) };
        }
    }

    // Called synchronously within the active Swift task. All Rust input was
    // initialized before task creation, so enqueue cannot race initialization.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn rust_authorization_prepare(root: *mut Root) -> *mut c_void {
        CALLER_ROOT.store(root.addr(), Ordering::Release);
        let input = INPUT.get().unwrap();
        let child = unsafe { swift_task_alloc(input.size) };
        assert!(!child.is_null());
        unsafe {
            child.cast::<Header>().write(Header {
                parent: root,
                resume_parent: rust_authorization_resume as *const c_void,
            });
            (*root).child = child;
            (*root).entry = input.entry as *const c_void;
            (*root).object = input.object as *mut c_void;
            (*root).member = input.member;
        }
        child
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn rust_authorization_finish(root: *mut Root, error: *mut c_void) {
        assert!(!root.is_null());
        assert_eq!(
            root.addr(),
            CALLER_ROOT.load(Ordering::Acquire),
            "async resume did not project the original caller frame"
        );
        println!("SDK async continuation caller frame validated");
        let child = unsafe { (*root).child };
        // The callee has returned to its parent; its task allocation is now the
        // last allocation. Release it before completing the caller task.
        unsafe { swift_task_dealloc(child) };
        let result = if error.is_null() {
            1
        } else {
            // Do not message an uninitialized native Swift error box. The
            // runtime bridge lazily creates its Cocoa domain/code first.
            let ns_error = unsafe { _swift_stdlib_bridgeErrorToNSError(error) };
            assert!(!ns_error.is_null());
            let domain = unsafe { objc_msgSend(ns_error, sel_registerName(c"domain".as_ptr())) };
            assert_ne!(domain, 0);
            let utf8 = unsafe {
                objc_msgSend(
                    domain as *mut c_void,
                    sel_registerName(c"UTF8String".as_ptr()),
                )
            };
            assert_ne!(utf8, 0);
            let domain = unsafe { std::ffi::CStr::from_ptr(utf8 as *const std::ffi::c_char) };
            let code =
                unsafe { objc_msgSend(ns_error, sel_registerName(c"code".as_ptr())) } as isize;
            // Only system error domain/code; no userInfo, application tokens,
            // account identity, or personal application data is read/logged.
            println!(
                "Public authorization NSError: domain={}, code={code}",
                domain.to_string_lossy()
            );
            unsafe { objc_release(ns_error) };
            2
        };
        RESULT.store(result, Ordering::Release);
    }

    // Swift 5.10 async convention on arm64: x22 context, x20 swiftself.
    // FamilyControlsMember is the SDK's @objc Swift.Int enum, passed in x0
    // with the C-compatible layout of GenEnum.cpp in Swift 5.10.
    // A Void throwing continuation receives its error in x20 and the callee
    // context in x22. Swift 5.10 GenCall.cpp emitAsyncReturn passes its CURRENT
    // context; the caller's continuation projects the parent before accessing
    // its own frame. Task.h's manual resumeParent helper is a different adapter.
    core::arch::global_asm!(
        ".text",
        ".p2align 2",
        ".globl _rust_authorization_entry",
        "_rust_authorization_entry:",
        "stp x29, x30, [sp, #-16]!",
        "mov x29, sp",
        "mov x0, x22",
        "bl _rust_authorization_prepare",
        "mov x17, x0",
        "ldr x16, [x22, #24]",
        "ldr x20, [x22, #32]",
        "ldr x0, [x22, #40]",
        "mov x22, x17",
        "ldp x29, x30, [sp], #16",
        "br x16",
        ".p2align 2",
        ".globl _rust_authorization_resume",
        "_rust_authorization_resume:",
        "stp x29, x30, [sp, #-16]!",
        "mov x29, sp",
        "ldr x22, [x22, #0]",
        "mov x0, x22",
        "mov x1, x20",
        "bl _rust_authorization_finish",
        "ldr x16, [x22, #8]",
        "mov x20, #0",
        "ldp x29, x30, [sp], #16",
        "br x16",
    );

    pub(super) fn start(object: *mut c_void) -> TaskOwner {
        let descriptor = &raw const REQUEST;
        let descriptor_value = unsafe { &*descriptor };
        assert!(descriptor_value.size >= 16 && descriptor_value.size <= 1_048_576);
        assert_ne!(descriptor_value.offset, 0);
        let entry = descriptor
            .cast::<u8>()
            .wrapping_offset(descriptor_value.offset as isize);
        assert_eq!(entry.addr() % 4, 0);
        let member: isize = env!("AUTH_INDIVIDUAL_VALUE").parse().unwrap();
        let input = Input {
            entry: entry.addr(),
            size: descriptor_value.size as usize,
            object: object.addr(),
            member,
        };
        assert!(INPUT.set(input).is_ok());
        assert_eq!(std::mem::size_of::<Header>(), 16);
        assert_eq!(std::mem::offset_of!(Root, child), 16);
        assert_eq!(std::mem::offset_of!(Root, entry), 24);
        assert_eq!(std::mem::offset_of!(Root, object), 32);
        assert_eq!(std::mem::offset_of!(Root, member), 40);
        let executor = unsafe { swift_task_getMainExecutor() };
        assert!(!executor.identity.is_null());
        // TaskOptionRecordKind::Executor = 0. The runtime reads the record
        // synchronously and copies ExecutorRef before enqueueing (Task.cpp).
        let option = ExecutorOption {
            flags: 0,
            parent: std::ptr::null(),
            executor,
        };
        println!("Public individual authorization request starting; member={member}");
        let task = unsafe {
            swift_task_create_common(
                1 << 12,
                (&raw const option).cast(),
                std::ptr::null(),
                rust_authorization_entry as *const c_void,
                std::ptr::null(),
                std::mem::size_of::<Root>(),
            )
        };
        assert!(!task.task.is_null() && !task.context.is_null());
        // Keep the create caller's reference until the bounded wait ends, so
        // cancellation on timeout never uses an already-deallocated task.
        TaskOwner(task.task)
    }

    #[cfg(not(uikit_host))]
    pub fn run(object: *mut c_void) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let owner = start(object);
        while RESULT.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 1) };
        }
        let result = RESULT.load(Ordering::Acquire);
        if result == 0 {
            owner.cancel();
            let cancel_deadline = Instant::now() + Duration::from_secs(5);
            while RESULT.load(Ordering::Acquire) == 0 && Instant::now() < cancel_deadline {
                unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 1) };
            }
            // A cancellation response is not an accepted authorization flow.
            panic!(
                "public authorization continuation timed out; cancellation requested; process will exit"
            );
        }
        assert_ne!(result, 0, "public authorization continuation timed out");
        println!(
            "Public individual authorization continuation observed; error_present={}",
            result == 2
        );
        println!("authorization_status_verified=false; monitoring=false; shielding=false");
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0) };
    }

    #[cfg(uikit_host)]
    #[cfg(authorization_status_consent_probe)]
    pub(super) fn completed_with_error() -> bool {
        let result = RESULT.load(Ordering::Acquire);
        assert!(
            result == 1 || result == 2,
            "authorization has not completed"
        );
        result == 2
    }

    #[cfg(uikit_host)]
    pub(super) fn wait_from_worker(owner: TaskOwner, started: Instant) -> bool {
        // The UI test runner has to launch the host before it can choose the
        // ordinary OS consent button. Keep the default inspection deadline.
        let wait_seconds = if cfg!(ui_test_host) { 90 } else { 20 };
        let deadline = started + Duration::from_secs(wait_seconds);
        while RESULT.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let result = RESULT.load(Ordering::Acquire);
        if result == 0 {
            owner.cancel();
            let deadline = Instant::now() + Duration::from_secs(5);
            while RESULT.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            eprintln!("UIKit authorization wait timed out; cancellation requested");
            return false;
        }
        println!(
            "Public individual authorization continuation observed; error_present={}",
            result == 2
        );
        println!("authorization_status_verified=false; monitoring=false; shielding=false");
        #[cfg(all(ui_test_host, not(authorization_status_consent_probe)))]
        {
            // Diagnostic output in this disposable test app's own container.
            // It contains no account details, tokens, or activity selection.
            let home = std::env::var_os("HOME").expect("test app container HOME missing");
            let documents = std::path::PathBuf::from(home).join("Documents");
            std::fs::create_dir_all(&documents).expect("create test output directory");
            std::fs::write(
                documents.join("public-authorization-result.txt"),
                format!("public_authorization_continuation_observed=true\nerror_present={}\nauthorization_status_verified=false\nmonitoring=false\nshielding=false\n", result == 2),
            ).expect("write public authorization test result");
        }
        true
    }
}

// This module is loaded only in Xcode's isolated XCTest UI test bundle. It
// performs public XCUIElement taps in the disposable Simulator, never in the
// product process or an owner's device. No authorization API is substituted.
#[cfg(ui_test_bundle)]
mod ui_authorization_test {
    use std::ffi::{c_char, c_void};

    type Id = *mut c_void;
    type Sel = *const c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn objc_allocateClassPair(parent: Id, name: *const c_char, extra: usize) -> Id;
        fn objc_registerClassPair(class: Id);
        fn class_addMethod(
            class: Id,
            selector: Sel,
            implementation: *const c_void,
            types: *const c_char,
        ) -> bool;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_retain(object: Id) -> Id;
        fn objc_release(object: Id);
    }

    // Xcode links XCTest into the runner's test bundle. The explicit framework
    // dependency makes its public Objective-C classes available at bundle load.
    #[link(name = "XCTest", kind = "framework")]
    unsafe extern "C" {}

    unsafe fn object(receiver: Id, selector: &std::ffi::CStr) -> Id {
        let send: unsafe extern "C" fn(Id, Sel) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, sel_registerName(selector.as_ptr())) }
    }

    unsafe fn object_arg(receiver: Id, selector: &std::ffi::CStr, argument: Id) -> Id {
        let send: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, sel_registerName(selector.as_ptr()), argument) }
    }

    unsafe fn action(receiver: Id, selector: &std::ffi::CStr) {
        let send: unsafe extern "C" fn(Id, Sel) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, sel_registerName(selector.as_ptr())) };
    }

    struct Owned(Id);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { objc_release(self.0) };
            }
        }
    }
    // Query/element getters return +0 objects. Keep a +1 reference while their
    // public wait/tap methods service the test runner's event loop.
    unsafe fn retained_object(receiver: Id, selector: &std::ffi::CStr) -> Owned {
        Owned(unsafe { objc_retain(object(receiver, selector)) })
    }
    unsafe fn retained_object_arg(receiver: Id, selector: &std::ffi::CStr, argument: Id) -> Owned {
        Owned(unsafe { objc_retain(object_arg(receiver, selector, argument)) })
    }

    unsafe fn string(text: &std::ffi::CStr) -> Owned {
        let class = unsafe { objc_getClass(c"NSString".as_ptr()) };
        let send: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        Owned(unsafe {
            send(
                object(class, c"alloc"),
                sel_registerName(c"initWithUTF8String:".as_ptr()),
                text.as_ptr(),
            )
        })
    }

    unsafe fn fail(test: Id, message: &std::ffi::CStr) {
        let description = unsafe { string(message) };
        let file = unsafe { string(c"ios-abi-probe.rs") };
        let send: unsafe extern "C" fn(Id, Sel, Id, Id, usize, bool) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe {
            send(
                test,
                sel_registerName(c"recordFailureWithDescription:inFile:atLine:expected:".as_ptr()),
                description.0,
                file.0,
                line!() as usize,
                true,
            )
        };
    }

    unsafe fn wait_for(element: Id, seconds: f64) -> bool {
        let send: unsafe extern "C" fn(Id, Sel, f64) -> bool =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe {
            send(
                element,
                sel_registerName(c"waitForExistenceWithTimeout:".as_ptr()),
                seconds,
            )
        }
    }

    unsafe fn describe_system(application: Id) {
        let description = unsafe { retained_object(application, c"debugDescription") };
        if !description.0.is_null() {
            let text = unsafe { object(description.0, c"UTF8String") }.cast::<c_char>();
            if !text.is_null() {
                println!(
                    "System UI observation: {}",
                    unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy()
                );
            }
        }
    }

    unsafe extern "C" fn choose_consent(test: Id, _: Sel) {
        unsafe {
            let application_class = objc_getClass(c"XCUIApplication".as_ptr());
            if application_class.is_null() {
                fail(test, c"Public XCUIApplication class missing");
                return;
            }
            let identifier = string(c"com.autodevjapan.phonecontrol1614abiprobe");
            let app = Owned(object_arg(
                object(application_class, c"alloc"),
                c"initWithBundleIdentifier:",
                identifier.0,
            ));
            if app.0.is_null() {
                fail(test, c"Cannot construct test application");
                return;
            }
            action(app.0, c"launch");
            let request_label = string(c"Request Screen Time access");
            let request_buttons = retained_object(app.0, c"buttons");
            let request_query =
                retained_object_arg(request_buttons.0, c"matchingIdentifier:", request_label.0);
            let request_button = retained_object(request_query.0, c"firstMatch");
            if !wait_for(request_button.0, 15.0) {
                fail(test, c"Test host request button not observed");
                return;
            }
            action(request_button.0, c"tap");
            // Family Controls presents a system remote alert. The inspected
            // app's AX tree contains only its request button (run 38075160045).
            // Observe SpringBoard through public XCTest; never launch it.
            let system_identifier = string(c"com.apple.springboard");
            let system_app = Owned(object_arg(
                object(application_class, c"alloc"),
                c"initWithBundleIdentifier:",
                system_identifier.0,
            ));
            if system_app.0.is_null() {
                fail(test, c"Cannot construct system UI observer");
                return;
            }
            let alerts = retained_object(system_app.0, c"alerts");
            let alert = retained_object(alerts.0, c"firstMatch");
            if !wait_for(alert.0, 30.0) {
                describe_system(system_app.0);
                fail(test, c"Screen Time consent alert not observed");
                return;
            }
            let label = string(if cfg!(ui_test_continue) {
                c"Continue"
            } else {
                c"Don’t Allow"
            });
            let alert_buttons = retained_object(alert.0, c"buttons");
            let button_query =
                retained_object_arg(alert_buttons.0, c"matchingIdentifier:", label.0);
            let mut button = retained_object(button_query.0, c"firstMatch");
            if !cfg!(ui_test_continue) && !wait_for(button.0, 0.1) {
                let ascii_label = string(c"Don't Allow");
                let ascii_query =
                    retained_object_arg(alert_buttons.0, c"matchingIdentifier:", ascii_label.0);
                button = retained_object(ascii_query.0, c"firstMatch");
            }
            if !wait_for(button.0, 5.0) {
                fail(test, c"Expected OS consent button not observed");
                return;
            }
            println!(
                "XCTest ordinary Screen Time consent button observed; continue={}",
                cfg!(ui_test_continue)
            );
            action(button.0, c"tap");
            println!(
                "XCTest ordinary Screen Time consent button tap returned; authorization_status_verified=false"
            );
            if cfg!(ui_test_continue) {
                // The fresh Simulator's observed second OS sheet is a separate
                // decision, not the result of pressing Continue (run 38076292581).
                let allow_label = string(c"Allow with Passcode");
                let system_buttons = retained_object(system_app.0, c"buttons");
                let allow_query =
                    retained_object_arg(system_buttons.0, c"matchingIdentifier:", allow_label.0);
                let allow_button = retained_object(allow_query.0, c"firstMatch");
                if !wait_for(allow_button.0, 10.0) {
                    describe_system(system_app.0);
                    fail(
                        test,
                        c"Observed second Screen Time authorization button missing",
                    );
                    return;
                }
                action(allow_button.0, c"tap");
                println!(
                    "XCTest second Screen Time authorization button tap returned; authorization_status_verified=false"
                );
                // Each run creates a new disposable Simulator. Use one non-secret
                // synthetic fixture through its ordinary OS authentication UI.
                let passcode_label = string(c"Passcode field");
                let fields = retained_object(system_app.0, c"secureTextFields");
                let field_query =
                    retained_object_arg(fields.0, c"matchingIdentifier:", passcode_label.0);
                let field = retained_object(field_query.0, c"firstMatch");
                if !wait_for(field.0, 10.0) {
                    describe_system(system_app.0);
                    fail(test, c"Observed Simulator passcode field missing");
                    return;
                }
                action(field.0, c"tap");
                let fixture = string(c"1614\n");
                let type_text: unsafe extern "C" fn(Id, Sel, Id) =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                type_text(field.0, sel_registerName(c"typeText:".as_ptr()), fixture.0);
                println!(
                    "XCTest disposable Simulator synthetic fixture submitted once; authorization_status_verified=false"
                );
                // The observed successful OS sheet still needs Done before
                // the public request returns (run38078299191).
                let done_label = string(c"Done");
                let finish_buttons = retained_object(system_app.0, c"buttons");
                let finish_query =
                    retained_object_arg(finish_buttons.0, c"matchingIdentifier:", done_label.0);
                let finish = retained_object(finish_query.0, c"firstMatch");
                if !wait_for(finish.0, 10.0) {
                    describe_system(system_app.0);
                    fail(
                        test,
                        c"Observed Simulator authorization completion button missing",
                    );
                    return;
                }
                action(finish.0, c"tap");
                println!(
                    "XCTest OS authorization completion button tap returned; authorization_status_verified=false"
                );
            }
            // This marker is set only after the host's public continuation and
            // its result file have been observed. A tap alone is insufficient.
            let result_label = string(c"Authorization result received");
            let result_buttons = retained_object(app.0, c"buttons");
            let result_query =
                retained_object_arg(result_buttons.0, c"matchingIdentifier:", result_label.0);
            let result_button = retained_object(result_query.0, c"firstMatch");
            if !wait_for(result_button.0, 30.0) {
                describe_system(system_app.0);
                fail(test, c"Public authorization continuation not observed");
                return;
            }
            println!("XCTest host public continuation marker observed");
            action(app.0, c"terminate");
        }
    }

    unsafe extern "C" fn register() {
        unsafe {
            let parent = objc_getClass(c"XCTestCase".as_ptr());
            if parent.is_null() {
                eprintln!("Public XCTestCase class missing at bundle load");
                std::process::exit(1);
            }
            let class = objc_allocateClassPair(parent, c"RustScreenTimeConsentTests".as_ptr(), 0);
            if class.is_null()
                || !class_addMethod(
                    class,
                    sel_registerName(c"testPermissionDecision".as_ptr()),
                    choose_consent as *const c_void,
                    c"v@:".as_ptr(),
                )
            {
                eprintln!("Cannot register isolated XCTest case");
                std::process::exit(1);
            }
            objc_registerClassPair(class);
        }
    }

    #[used]
    #[unsafe(link_section = "__DATA,__mod_init_func,mod_init_funcs")]
    static REGISTER: unsafe extern "C" fn() = register;
}

#[cfg(all(runtime_probe, uikit_host, auth_request))]
mod native_host {
    use super::request_probe::objc_msgSend;
    use std::ffi::{CString, c_char, c_void};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;
    #[cfg(not(selection_layout_probe))]
    use std::time::Instant;
    type Id = *mut c_void;
    type Sel = *const c_void;
    #[derive(Clone, Copy)]
    #[repr(C)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static WINDOW: AtomicUsize = AtomicUsize::new(0);
    static DELEGATE: AtomicUsize = AtomicUsize::new(0);
    #[cfg(ui_test_host)]
    static REQUESTED: AtomicBool = AtomicBool::new(false);
    #[cfg(ui_test_host)]
    static REQUEST_BUTTON: AtomicUsize = AtomicUsize::new(0);
    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {
        fn UIApplicationMain(argc: i32, argv: *mut *mut c_char, principal: Id, delegate: Id)
        -> i32;
    }
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn objc_getProtocol(name: *const c_char) -> Id;
        fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
        fn objc_registerClassPair(class: Id);
        fn class_addProtocol(class: Id, protocol: Id) -> bool;
        fn class_addMethod(
            class: Id,
            selector: Sel,
            implementation: *const c_void,
            types: *const c_char,
        ) -> bool;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_retain(object: Id) -> Id;
        #[cfg(ui_test_host)]
        fn objc_release(object: Id);
    }
    #[link(name = "System")]
    unsafe extern "C" {
        // dispatch/queue.h exposes dispatch_get_main_queue as a C inline
        // function returning DISPATCH_GLOBAL_OBJECT(..., _dispatch_main_q).
        // Only take the address of this opaque global; never inspect its layout.
        static _dispatch_main_q: [u8; 0];
        fn dispatch_async_f(
            queue: Id,
            context: *mut c_void,
            function: unsafe extern "C" fn(*mut c_void),
        );
    }
    unsafe fn send0(object: Id, name: &std::ffi::CStr) -> Id {
        let f: unsafe extern "C" fn(Id, Sel) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize) };
        unsafe { f(object, sel_registerName(name.as_ptr())) }
    }
    unsafe fn send_id(object: Id, name: &std::ffi::CStr, value: Id) {
        let f: unsafe extern "C" fn(Id, Sel, Id) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize) };
        unsafe { f(object, sel_registerName(name.as_ptr()), value) };
    }
    unsafe fn send_void(object: Id, name: &std::ffi::CStr) {
        let f: unsafe extern "C" fn(Id, Sel) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize) };
        unsafe { f(object, sel_registerName(name.as_ptr())) };
    }
    #[cfg(ui_test_host)]
    unsafe fn test_button_title(button: Id, title: &std::ffi::CStr) {
        unsafe {
            let make_string: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize);
            let string = make_string(
                send0(objc_getClass(c"NSString".as_ptr()), c"alloc"),
                sel_registerName(c"initWithUTF8String:".as_ptr()),
                title.as_ptr(),
            );
            let set_title: unsafe extern "C" fn(Id, Sel, Id, usize) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize);
            set_title(
                button,
                sel_registerName(c"setTitle:forState:".as_ptr()),
                string,
                0,
            );
            send_id(button, c"setAccessibilityIdentifier:", string);
            objc_release(string);
        }
    }
    #[cfg(ui_test_host)]
    unsafe extern "C" fn completed_on_main(_: *mut c_void) {
        #[cfg(authorization_status_consent_probe)]
        {
            let is_main: unsafe extern "C" fn(Id, Sel) -> bool = unsafe {
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize)
            };
            assert!(
                unsafe {
                    is_main(
                        objc_getClass(c"NSThread".as_ptr()),
                        sel_registerName(c"isMainThread".as_ptr()),
                    )
                },
                "status must be read on the main thread"
            );
            let object = super::inspect_singleton();
            let metadata = super::authorization_status_entry::inspect(object);
            let status = super::authorization_status_entry::read_value(object, metadata);
            let error = super::request_probe::completed_with_error();
            let home = std::env::var_os("HOME").expect("test app container HOME missing");
            let documents = std::path::PathBuf::from(home).join("Documents");
            std::fs::create_dir_all(&documents).expect("create test output directory");
            std::fs::write(
                documents.join("public-authorization-result.txt"),
                format!("public_authorization_continuation_observed=true\nerror_present={error}\nauthorization_status_verified=true\nstatus_raw_value={}\nstatus_label={}\nstatus_getter_calls=1\nraw_value_getter_calls=1\nenum_case_name_calls=1\nstatus_read_on_main_thread=true\nmonitoring=false\nshielding=false\n", status.raw, status.label),
            ).expect("write public authorization and status test result");
        }
        unsafe {
            test_button_title(
                REQUEST_BUTTON.load(Ordering::Acquire) as Id,
                c"Authorization result received",
            )
        };
    }
    #[cfg(ui_test_host)]
    unsafe extern "C" fn request_clicked(_: Id, _: Sel, _: Id) {
        if !REQUESTED.swap(true, Ordering::AcqRel) {
            start_request(super::inspect_singleton());
        }
    }
    unsafe extern "C" fn window(_: Id, _: Sel) -> Id {
        WINDOW.load(Ordering::Acquire) as Id
    }
    unsafe extern "C" fn launched(delegate: Id, _: Sel, _: Id, _: Id) -> bool {
        assert!(!delegate.is_null());
        DELEGATE.store(unsafe { objc_retain(delegate) }.addr(), Ordering::Release);
        let screen = unsafe { send0(objc_getClass(c"UIScreen".as_ptr()), c"mainScreen") };
        assert!(!screen.is_null());
        let bounds: unsafe extern "C" fn(Id, Sel) -> Rect =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize) };
        let rect = unsafe { bounds(screen, sel_registerName(c"bounds".as_ptr())) };
        assert!(rect.width > 0.0 && rect.height > 0.0);
        let allocated = unsafe { send0(objc_getClass(c"UIWindow".as_ptr()), c"alloc") };
        let init: unsafe extern "C" fn(Id, Sel, Rect) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize) };
        let view_window = unsafe {
            init(
                allocated,
                sel_registerName(c"initWithFrame:".as_ptr()),
                rect,
            )
        };
        assert!(!view_window.is_null());
        WINDOW.store(view_window.addr(), Ordering::Release);
        let controller = unsafe {
            send0(
                send0(objc_getClass(c"UIViewController".as_ptr()), c"alloc"),
                c"init",
            )
        };
        assert!(!controller.is_null());
        #[cfg(ui_test_host)]
        unsafe {
            let create: unsafe extern "C" fn(Id, Sel, isize) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize);
            let button = create(
                objc_getClass(c"UIButton".as_ptr()),
                sel_registerName(c"buttonWithType:".as_ptr()),
                env!("UI_SYSTEM_BUTTON_TYPE").parse().unwrap(),
            );
            assert!(!button.is_null());
            let set_frame: unsafe extern "C" fn(Id, Sel, Rect) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize);
            set_frame(
                button,
                sel_registerName(c"setFrame:".as_ptr()),
                Rect {
                    x: 20.0,
                    y: 120.0,
                    width: rect.width - 40.0,
                    height: 60.0,
                },
            );
            test_button_title(button, c"Request Screen Time access");
            let add_target: unsafe extern "C" fn(Id, Sel, Id, Sel, usize) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize);
            add_target(
                button,
                sel_registerName(c"addTarget:action:forControlEvents:".as_ptr()),
                delegate,
                sel_registerName(c"requestScreenTime:".as_ptr()),
                env!("UI_TOUCH_UP_INSIDE_EVENT").parse().unwrap(),
            );
            send_id(send0(controller, c"view"), c"addSubview:", button);
            REQUEST_BUTTON.store(button.addr(), Ordering::Release);
        }
        unsafe {
            send_id(view_window, c"setRootViewController:", controller);
            send_void(view_window, c"makeKeyAndVisible");
        }
        // Keep the allocation references for this bounded inspection process.
        #[cfg(not(ui_test_host))]
        println!("Rust UIKit launch callback observed; blank_window=true");
        #[cfg(ui_test_host)]
        println!("Rust UIKit launch callback observed; test_request_button=true");
        true
    }
    unsafe extern "C" fn exit_on_main(context: *mut c_void) {
        std::process::exit(context.addr() as i32);
    }
    unsafe extern "C" fn active(_: Id, _: Sel, _: Id) {
        if ACTIVE.swap(true, Ordering::AcqRel) {
            return;
        }
        println!("Rust UIKit active callback observed");
        #[cfg(selection_layout_probe)]
        {
            // Keep the getter's allocation reference for this bounded process,
            // as with its window/controller. This mode starts no Swift task.
            // Catch Rust validation failures before crossing the C callback;
            // value owners unwind and emit their partial lifecycle counts.
            let success = std::panic::catch_unwind(|| {
                let _ = super::inspect_singleton();
                let metadata = super::selection_layout::inspect();
                #[cfg(selection_value_probe)]
                super::selection_value::inspect(metadata);
                #[cfg(mutable_binding_probe)]
                super::mutable_binding::inspect(metadata);
                #[cfg(not(any(selection_value_probe, mutable_binding_probe)))]
                let _ = metadata;
            })
            .is_ok();
            unsafe {
                dispatch_async_f(
                    (&raw const _dispatch_main_q).cast_mut().cast(),
                    std::ptr::without_provenance_mut(if success { 0 } else { 1 }),
                    exit_on_main,
                )
            };
        }
        #[cfg(all(not(ui_test_host), not(selection_layout_probe)))]
        start_request(super::inspect_singleton());
        #[cfg(ui_test_host)]
        println!("Rust XCTest host ready; authorization_requested=false");
    }
    #[cfg(not(selection_layout_probe))]
    fn start_request(object: Id) {
        let started = Instant::now();
        let owner = super::request_probe::start(object);
        // Return to UIKit immediately; its main executor must remain available
        // to run the Swift task. The worker only polls an atomic result and
        // cancels a retained task; it never accesses UIKit or task allocations.
        std::thread::spawn(move || {
            let success = super::request_probe::wait_from_worker(owner, started);
            #[cfg(ui_test_host)]
            if success {
                unsafe {
                    dispatch_async_f(
                        (&raw const _dispatch_main_q).cast_mut().cast(),
                        std::ptr::null_mut(),
                        completed_on_main,
                    );
                }
                // XCTest terminates this disposable host after observing the
                // result marker. Keep it alive during the ordinary OS tap.
                return;
            }
            let code = if success { 0 } else { 1 };
            unsafe {
                dispatch_async_f(
                    (&raw const _dispatch_main_q).cast_mut().cast(),
                    std::ptr::without_provenance_mut(code),
                    exit_on_main,
                )
            };
        });
    }
    pub(super) fn run() {
        let name = c"PhoneControl1614ProbeDelegate";
        unsafe {
            let class =
                objc_allocateClassPair(objc_getClass(c"NSObject".as_ptr()), name.as_ptr(), 0);
            assert!(!class.is_null());
            assert!(class_addProtocol(
                class,
                objc_getProtocol(c"UIApplicationDelegate".as_ptr())
            ));
            assert!(class_addMethod(
                class,
                sel_registerName(c"application:didFinishLaunchingWithOptions:".as_ptr()),
                launched as *const c_void,
                c"B@:@@".as_ptr()
            ));
            assert!(class_addMethod(
                class,
                sel_registerName(c"applicationDidBecomeActive:".as_ptr()),
                active as *const c_void,
                c"v@:@".as_ptr()
            ));
            #[cfg(ui_test_host)]
            assert!(class_addMethod(
                class,
                sel_registerName(c"requestScreenTime:".as_ptr()),
                request_clicked as *const c_void,
                c"v@:@".as_ptr()
            ));
            assert!(class_addMethod(
                class,
                sel_registerName(c"window".as_ptr()),
                window as *const c_void,
                c"@@:".as_ptr()
            ));
            objc_registerClassPair(class);
            let string_class = objc_getClass(c"NSString".as_ptr());
            let init_string: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn(Id, Sel) -> usize);
            let delegate_name = init_string(
                send0(string_class, c"alloc"),
                sel_registerName(c"initWithUTF8String:".as_ptr()),
                name.as_ptr(),
            );
            assert!(!delegate_name.is_null());
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(30));
                #[cfg(selection_layout_probe)]
                {
                    // Also bound a stalled foreign initializer/witness call.
                    eprintln!("Selection inspection process timed out");
                    std::process::exit(1);
                }
                #[cfg(not(selection_layout_probe))]
                if !ACTIVE.load(Ordering::Acquire) {
                    eprintln!("UIKit active callback timed out");
                    std::process::exit(1);
                }
            });
            let executable = CString::new("authorization-center-probe").unwrap();
            let mut argv = [executable.as_ptr().cast_mut(), std::ptr::null_mut()];
            let result =
                UIApplicationMain(1, argv.as_mut_ptr(), std::ptr::null_mut(), delegate_name);
            panic!("UIApplicationMain unexpectedly returned {result}");
        }
    }
}
