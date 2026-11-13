import XCTest
import FamilyControls
import Foundation

@available(iOS 16.0, *)
@MainActor
final class SelectionCodecTests: XCTestCase {
    func testPublicCodecRoundtrip() throws {
        let value = FamilyActivitySelection()
        let encoded = try SelectionCodec.encode(value)
        XCTAssertFalse(encoded.isEmpty)
        XCTAssertEqual(try SelectionCodec.decode(encoded), value)
        let bridged = try SelectionCodec.encodeNSData(value)
        XCTAssertEqual(try SelectionCodec.decode(bridged), value)
    }

    func testInvalidSavedDataDoesNotPublishDefaults() {
        var calls = 0
        XCTAssertThrowsError(try RuntimeSelectionSession(
            savedData: Data([0xff]),
            publish: { _, _ in calls += 1 }
        ))
        XCTAssertEqual(calls, 0)
    }

    func testUncertainPublicationBlocksReuseAndRequiresReconstruction() throws {
        enum Failure: Error { case afterPublication }
        var disk: Data?
        var calls = 0
        let session = try RuntimeSelectionSession(savedData: nil) { _, data in
            calls += 1
            disk = data
            throw Failure.afterPublication
        }
        XCTAssertThrowsError(try session.commit(FamilyActivitySelection()))
        XCTAssertTrue(session.recoveryRequired)
        XCTAssertThrowsError(try session.commit(FamilyActivitySelection()))
        XCTAssertEqual(calls, 1)
        let reopened = try RuntimeSelectionSession(savedData: disk) { _, _ in }
        XCTAssertFalse(reopened.recoveryRequired)
        XCTAssertEqual(reopened.selection, try SelectionCodec.decode(XCTUnwrap(disk)))
    }
}
