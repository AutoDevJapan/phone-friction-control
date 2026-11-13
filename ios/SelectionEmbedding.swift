import Foundation
import FamilyControls

/// Embedding ownership for an admitted native session. The application linker
/// must supply verified implementations; ordinary startup does not instantiate
/// this candidate. No session address escapes into a copied Binding.
@available(iOS 16.0, *)
@MainActor
final class SelectionEmbedding {
    struct Operations {
        let open: (URL) throws -> UnsafeMutableRawPointer
        // Owned encoded bytes, or nil only for a genuinely absent selection.
        let restored: (UnsafeMutableRawPointer) throws -> Data?
        // Consumes the Selection +1 on every outcome, including failure.
        let commit: (UnsafeMutableRawPointer, UnsafeMutableRawPointer) -> Bool
        let release: (UnsafeMutableRawPointer) -> Void
    }

    private var session: UnsafeMutableRawPointer?
    private let operations: Operations
    private let restored: FamilyActivitySelection

    init(path: URL, admittedOperations: Operations) throws {
        operations = admittedOperations
        let opened = try admittedOperations.open(path)
        do {
            if let data = try admittedOperations.restored(opened) {
                restored = try SelectionCodec.decode(data)
            } else {
                restored = FamilyActivitySelection()
            }
            session = opened
        } catch {
            admittedOperations.release(opened)
            throw error
        }
    }

    func makePickerModel() -> SelectionPickerModel {
        SelectionPickerModel(restored: restored) { [self] ownedSelection in
            guard let session else {
                // Rejected callbacks still consume the transferred ownership.
                selectionRelease(ownedSelection)
                return false
            }
            return operations.commit(session, ownedSelection)
        }
    }

    /// Called on the main thread after closing the presentation. Binding copies
    /// can outlive close; their later commits are refused without session use.
    func close() {
        guard let owned = session else { return }
        session = nil
        operations.release(owned)
    }

    deinit {
        // The app embedding must explicitly close while main-actor isolated;
        // do not silently release a thread-bound Rust session from deinit.
        precondition(session == nil, "SelectionEmbedding must close before release")
    }
}
