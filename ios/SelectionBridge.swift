import Foundation
import FamilyControls

@available(iOS 16.0, *)
private final class OwnedSelection {
    let value: FamilyActivitySelection
    init(_ value: FamilyActivitySelection) { self.value = value }
}

// Only bridge-produced handles are accepted. Arbitrary addresses cannot be
// validated by Unmanaged; callers must preserve the header's ownership contract.
@available(iOS 16.0, *)
@_cdecl("pfc_selection_decode")
func selectionDecode(_ bytes: UnsafePointer<UInt8>?, _ count: Int) -> UnsafeMutableRawPointer? {
    guard Thread.isMainThread, count > 0, let bytes else { return nil }
    do {
        let value = try SelectionCodec.decode(Data(bytes: bytes, count: count))
        return Unmanaged.passRetained(OwnedSelection(value)).toOpaque()
    } catch { return nil }
}

@available(iOS 16.0, *)
@_cdecl("pfc_selection_encode")
func selectionEncode(
    _ handle: UnsafeMutableRawPointer?, _ context: UnsafeMutableRawPointer?,
    _ sink: (@convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Int32)?
) -> Int32 {
    guard Thread.isMainThread, let handle, let sink else { return -1 }
    let selection = Unmanaged<OwnedSelection>.fromOpaque(handle).takeUnretainedValue()
    do {
        let data = try SelectionCodec.encode(selection.value)
        return data.withUnsafeBytes { buffer in
            sink(context, buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count)
        }
    } catch { return -2 }
}

@available(iOS 16.0, *)
@_cdecl("pfc_selection_equal")
func selectionEqual(_ left: UnsafeMutableRawPointer?, _ right: UnsafeMutableRawPointer?) -> Int32 {
    guard Thread.isMainThread, let left, let right else { return -1 }
    let a = Unmanaged<OwnedSelection>.fromOpaque(left).takeUnretainedValue()
    let b = Unmanaged<OwnedSelection>.fromOpaque(right).takeUnretainedValue()
    return a.value == b.value ? 1 : 0
}

@available(iOS 16.0, *)
@_cdecl("pfc_selection_release")
func selectionRelease(_ handle: UnsafeMutableRawPointer?) {
    precondition(Thread.isMainThread)
    guard let handle else { return }
    Unmanaged<OwnedSelection>.fromOpaque(handle).release()
}
