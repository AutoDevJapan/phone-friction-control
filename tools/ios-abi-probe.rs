//! Default mode: compiler experiment with an undefined external symbol.
//! runtime_probe mode: isolated Simulator getter and async runtime inspection.
//! auth_request additionally calls the public individual authorization API once.
#![cfg_attr(not(runtime_probe), no_std)]
#![feature(abi_swift)]

#[cfg(all(
    runtime_probe,
    not(all(target_os = "ios", target_arch = "aarch64", target_abi = "sim"))
))]
compile_error!("runtime inspection is limited to arm64 iOS Simulator");

#[cfg(not(runtime_probe))]
unsafe extern "Swift" {
    fn abi_probe_external();
}

#[unsafe(no_mangle)]
#[cfg(not(runtime_probe))]
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
fn main() {
    // Getter inspection alone does not request authorization or read app data.
    // The optional auth_request stage below is a separate one-shot OS request.
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
    async_inspection::inspect();
    #[cfg(auth_request)]
    request_probe::run(object);
    // Keep the singleton reference until this isolated process ends.
}

#[cfg(runtime_probe)]
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
        fn objc_msgSend(object: *mut c_void, selector: *const c_void) -> usize;
        fn objc_release(object: *mut c_void);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }
    unsafe extern "C" {
        fn rust_authorization_entry();
        fn rust_authorization_resume();
    }
    struct TaskOwner(*mut c_void);
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

    pub fn run(object: *mut c_void) {
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
        let _owner = TaskOwner(task.task);
        let deadline = Instant::now() + Duration::from_secs(20);
        while RESULT.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 1) };
        }
        let result = RESULT.load(Ordering::Acquire);
        if result == 0 {
            unsafe { swift_task_cancel(task.task) };
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
}
