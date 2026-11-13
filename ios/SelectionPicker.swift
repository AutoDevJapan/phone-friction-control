import FamilyControls
import SwiftUI

/// Source-only wiring for the existing system Picker. The callback consumes the
/// supplied +1 handle on every result, including failure, without throwing.
/// The embedding owner must keep the Rust session alive until this model and
/// all Picker Binding copies have been released.
@available(iOS 16.0, *)
@MainActor
final class SelectionPickerModel: ObservableObject {
    @Published private(set) var selection: FamilyActivitySelection
    private(set) var recoveryRequired = false
    private let commitOwned: (UnsafeMutableRawPointer) -> Bool

    init(
        restored: FamilyActivitySelection,
        commitOwned: @escaping (UnsafeMutableRawPointer) -> Bool
    ) {
        selection = restored
        self.commitOwned = commitOwned
    }

    var binding: Binding<FamilyActivitySelection> {
        Binding(
            get: { self.selection },
            set: { proposed in
                guard !self.recoveryRequired else { return }
                let owned = retainSelectionForRuntime(proposed)
                guard self.commitOwned(owned) else {
                    self.recoveryRequired = true
                    return
                }
                self.selection = proposed
            }
        )
    }
}

@available(iOS 16.0, *)
@MainActor
struct RuntimeSelectionPicker: View {
    @ObservedObject var model: SelectionPickerModel
    var body: some View {
        FamilyActivityPicker(selection: model.binding)
    }
}
