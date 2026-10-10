//! Default mode: compiler experiment with an undefined external symbol.
//! runtime_probe mode: isolated Simulator getter and async runtime inspection.
//! Authorization requests require the explicit auth_request mode.
#![cfg_attr(not(any(runtime_probe, ui_test_bundle)), no_std)]
#![cfg_attr(not(ui_test_bundle), feature(abi_swift))]
#![cfg_attr(xcode_host, no_main)]

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
        async_inspection::inspect();
        #[cfg(auth_request)]
        request_probe::run(object);
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

#[cfg(runtime_probe)]
#[cfg_attr(uikit_host, allow(dead_code))]
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
        #[cfg(ui_test_host)]
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
    use std::time::{Duration, Instant};
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
        #[cfg(not(ui_test_host))]
        start_request(super::inspect_singleton());
        #[cfg(ui_test_host)]
        println!("Rust XCTest host ready; authorization_requested=false");
    }
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
