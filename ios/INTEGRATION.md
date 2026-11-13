# Selection codec integration candidate

The Swift files are unlinked source candidates. `SelectionCodec` uses the public
`FamilyActivitySelection` Codable and Equatable conformances. Data is returned
as an owned value; no temporary `withUnsafeBytes` pointer is exported.

`RuntimeSelectionStore` binds the Rust StateStore to one codec instance. Opening
restores and validates existing bytes before publishing is possible. Commit
encodes, decodes, compares the decoded selection, and only then saves. An
uncertain write blocks the existing instance; reconstruction requires another
read and successful decode.

Not connected: the native implementation of the Rust codec trait, Data/NSData
cross-language ownership bridge, build/link configuration and Runtime picker
callsite. No foreign signature is inferred from the Swift source. The current
Runtime remains unchanged and cannot select, monitor or shield applications.

Required Apple validation: compile these sources and tests with the same pinned
SDK as the native Selection host; inspect the actual encode/decode and Data
lowering before introducing a Rust foreign call. Then run a nonempty selected
value through encode, decode/equality, checked publication, termination and
restoration. Empty-selection and synthetic host tests cannot replace that run.

Local Swift compiler and Apple runtime validation have not been performed.
The existing authentication stop and workflow permission restriction remain in
effect. These source candidates do not establish an additional build service.
