import Foundation
import FamilyControls

/// Keeps opaque tokens in their native Swift representation. The returned Data
/// is owned; no pointer into a temporary Data/NSData allocation escapes.
@available(iOS 16.0, *)
enum SelectionCodec {
    static func encode(_ selection: FamilyActivitySelection) throws -> Data {
        let data = try JSONEncoder().encode(selection)
        guard !data.isEmpty,
              try decode(data) == selection else {
            throw CodecError.roundtripMismatch
        }
        return data
    }

    static func decode(_ data: Data) throws -> FamilyActivitySelection {
        try JSONDecoder().decode(FamilyActivitySelection.self, from: data)
    }

    enum CodecError: Error {
        case roundtripMismatch
        case recoveryRequired
    }
}

/// Source integration boundary. A product adapter must supply the existing
/// checked storage operations; this class does not create a second file store.
/// Instantiate only after the storage adapter's initial read succeeds.
@available(iOS 16.0, *)
@MainActor
final class RuntimeSelectionSession {
    private(set) var selection: FamilyActivitySelection
    private(set) var recoveryRequired = false
    private let publish: (FamilyActivitySelection, Data) throws -> Void

    init(
        savedData: Data?,
        publish: @escaping (FamilyActivitySelection, Data) throws -> Void
    ) throws {
        // A failed decode throws before an instance or default selection can
        // replace the previous stored value.
        selection = try savedData.map(SelectionCodec.decode)
            ?? FamilyActivitySelection()
        self.publish = publish
    }

    func commit(_ proposed: FamilyActivitySelection) throws {
        guard !recoveryRequired else {
            throw SelectionCodec.CodecError.recoveryRequired
        }
        let data = try SelectionCodec.encode(proposed)
        do {
            try publish(proposed, data)
        } catch {
            // Publication may have renamed the file before returning an error.
            // Reopen and decode through a new session; never reuse old memory.
            recoveryRequired = true
            throw error
        }
        selection = proposed
    }
}
