# Selection codec integration candidate

The Swift files are unlinked source candidates. `SelectionCodec` uses the public
`FamilyActivitySelection` Codable and Equatable conformances. Data is returned
as an owned value; no temporary `withUnsafeBytes` pointer is exported.

`RuntimeSelectionStore` binds the Rust StateStore to one codec instance. Opening
restores and validates existing bytes before publishing is possible. Commit
encodes, decodes, compares the decoded selection, and only then saves. An
uncertain write blocks the existing instance; reconstruction requires another
read and successful decode.

The candidate C bridge owns Swift Selection handles and synchronously copies
encoded bytes into Rust. The Rust codec consumer and Runtime session use a
verified-provider function table. `SelectionPickerModel` wires the existing
system Picker setter to a consuming commit callback, retaining the new Swift
value only after successful publication. The callback must consume the handle
on every result; the embedding owner must outlive every copied Binding.

Not connected: build/link configuration, verified provider admission, actual
app hosting and lifetime ownership. No ordinary Runtime startup enables these
candidates; it cannot select, monitor or shield applications yet.

Required Apple validation: compile these sources and tests with the same pinned
SDK as the native Selection host; inspect the actual encode/decode and Data
lowering before introducing a Rust foreign call. Generate the Swift Objective-C
header with `-emit-objc-header` and compare the exported C declarations and
integer widths with SelectionBridge.h and NativeApi; compilation must reject
any mismatch. This is an existing compiler-job dependency, not permission to
change workflow authentication or enable untested calls. Then run a nonempty selected
value through encode, decode/equality, checked publication, termination and
restoration. Empty-selection and synthetic host tests cannot replace that run.

Local Swift compiler and Apple runtime validation have not been performed.
The existing authentication stop and workflow permission restriction remain in
effect. These source candidates do not establish an additional build service.

## Compiler handoff

Historical input `645780a8068e0685f02cd51f5d58ce84ddd39277` lacked generated
ABI comparison and is not an executable acceptance input. The current compiler
input is the full product commit and driver blob asserted by the SDK caller;
after each repair both caller pins must be updated before execution. SDK896
still pins the older cbff version and must not be run as acceptance of repairs.
Use tracked source bytes and Cargo.lock from that caller-pinned commit, not a
working-tree copy. Swift inputs are SelectionCodec.swift, SelectionBridge.swift,
SelectionPicker.swift, SelectionEmbedding.swift, SelectionSessionAdapter.swift
and SelectionCodecTests.swift. C inputs are SelectionBridge.h,
SelectionSession.h and SelectionBridgeABI.c. Rust consumers are
src/ios_selection_bridge.rs, src/ios_selection_session_ffi.rs,
src/ios_selection_storage.rs and src/ios_runtime.rs; src/lib.rs supplies cfg
connections. The compiler driver itself is ios/compile-selection-codec.sh.

The existing authorized Apple compiler job must preserve compiler/SDK versions,
target triple, source hashes, full diagnostics, generated Objective-C header,
exported symbol list and its exact command lines. Typecheck production Swift
inputs first, then compile the test input in the same module; compile a C
translation unit against the actual generated declarations as well as the
handwritten header. If C exports are absent from the generated header, report
that absence and retain compiler IR/C export definitions rather than claiming
a comparison succeeded. No such compiler job has run for this candidate.

Connection gap: RuntimeSelectionPicker invokes an injected Swift closure, while
NativeSelectionSession consumes a Rust owned handle. No app embedding currently
supplies that closure with a verified provider/session lifetime. No linked C
symbol resolver or raw Rust session pointer is enabled. The app owner must keep
the session alive through all Binding copies, close callbacks before teardown,
then demonstrate restoration using the real codec. Source unit tests with a
synthetic provider do not certify that embedding or the native destructor path.

## Existing workflow connection

The SDK compiler caller must check out the product into a separate subdirectory
at an explicitly supplied full commit SHA, verify `git rev-parse HEAD` against
that SHA, and invoke `bash ios/compile-selection-codec.sh` from that product
checkout. The output argument must be a new directory under the existing runner
temporary directory. Upload that entire output directory with `if: always()`;
preserve a failed stage's status/diagnostics and do not replace its exit code
with the upload result. Include this driver and both C headers in the input hash
manifest. The existing SDK ownership compiler job and its old outputs remain a
separate mode: no existing successful inspection is relabeled as codec proof.

This specifies the producer/consumer wiring only. The original SDK workflow is
not modified by this product documentation; its exact source access and remote
OAuth workflow scope remain separate dependencies. No additional remote run or
authentication action is authorized or performed by this file.
