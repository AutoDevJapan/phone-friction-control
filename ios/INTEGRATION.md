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
