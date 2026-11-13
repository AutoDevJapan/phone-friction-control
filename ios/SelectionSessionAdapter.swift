import Foundation

private final class RestoredBytes {
    var data: Data?
    var rejected = false
}

private let receiveSelectionBytes: @convention(c) (
    UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int
) -> Int32 = { context, bytes, count in
    guard let context else { return -1 }
    let state = Unmanaged<RestoredBytes>.fromOpaque(context).takeUnretainedValue()
    guard state.data == nil, let bytes, count > 0 else {
        state.rejected = true
        return -1
    }
    state.data = Data(bytes: bytes, count: count)
    return 0
}

/// Compile with SelectionSession.h as the bridging header. These direct calls
/// remain unlinked until the pinned Swift/C/Rust compiler checks succeed.
@available(iOS 16.0, *)
@MainActor
extension SelectionEmbedding.Operations {
    static func native() -> Self {
        Self(
            open: { url in
                guard url.isFileURL else { throw CocoaError(.fileReadUnsupportedScheme) }
                var api = pfc_selection_api(
                    decode: selectionDecode, encode: selectionEncode,
                    equal: selectionEqual, release: selectionRelease
                )
                let path = Array(url.path.utf8)
                let opened = path.withUnsafeBufferPointer { buffer in
                    pfc_selection_session_open(buffer.baseAddress, buffer.count, &api)
                }
                guard let opened else { throw CocoaError(.fileReadUnknown) }
                return opened
            },
            restored: { session in
                let state = RestoredBytes()
                let context = Unmanaged.passUnretained(state).toOpaque()
                let status = pfc_selection_session_restored(session, context, receiveSelectionBytes)
                if status == 1 && state.data == nil && !state.rejected { return nil }
                guard status == 0, let data = state.data, !state.rejected else {
                    throw CocoaError(.fileReadCorruptFile)
                }
                return data
            },
            commit: { session, selected in
                pfc_selection_session_commit(session, selected) == 0
            },
            release: { session in pfc_selection_session_release(session) }
        )
    }
}
