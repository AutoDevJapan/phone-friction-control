import XCTest
import FamilyControls
import Foundation
import SwiftUI

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

    func testCopiedBindingKeepsClosedEmbeddingUntilItsFinalCopyIsReleased() throws {
        let probe = EmbeddingOperationsProbe()
        var embedding: SelectionEmbedding? = try SelectionEmbedding(
            path: URL(fileURLWithPath: "/fixture/selection.ron"),
            admittedOperations: probe.operations()
        )
        defer { embedding?.close() }
        weak var weakEmbedding = embedding
        var model: SelectionPickerModel? = embedding!.makePickerModel()
        weak var weakModel = model
        var original: Binding<FamilyActivitySelection>? = model!.binding
        var copy = original

        let initialFlag = copy!.wrappedValue.includeEntireCategory
        let committed = FamilyActivitySelection(
            includeEntireCategory: !initialFlag
        )
        XCTAssertNotEqual(committed.includeEntireCategory, initialFlag)
        copy!.wrappedValue = committed
        XCTAssertEqual(copy!.wrappedValue.includeEntireCategory, committed.includeEntireCategory)
        XCTAssertEqual(probe.commits, 1)
        XCTAssertEqual(probe.candidateReleaseCalls, 1)
        XCTAssertFalse(model!.recoveryRequired)
        XCTAssertEqual(probe.releases, 0)
        XCTAssertNotNil(probe.session)

        embedding!.close()
        embedding!.close()
        XCTAssertEqual(probe.releases, 1)
        XCTAssertEqual(probe.sessionDestructions, 1)
        XCTAssertNil(probe.session)

        embedding = nil
        model = nil
        original = nil
        XCTAssertNotNil(weakEmbedding)
        XCTAssertNotNil(weakModel)
        XCTAssertEqual(copy!.wrappedValue, committed)

        // A copied Binding can outlive close, but cannot use the retired session.
        let rejected = FamilyActivitySelection(
            includeEntireCategory: !committed.includeEntireCategory
        )
        XCTAssertNotEqual(rejected.includeEntireCategory, committed.includeEntireCategory)
        copy!.wrappedValue = rejected
        XCTAssertEqual(copy!.wrappedValue.includeEntireCategory, committed.includeEntireCategory)
        XCTAssertEqual(probe.commits, 1)
        XCTAssertEqual(probe.invalidSessionUses, 0)
        XCTAssertEqual(probe.releases, 1)
        XCTAssertTrue(weakModel!.recoveryRequired)

        copy = nil
        XCTAssertNil(weakModel)
        XCTAssertNil(weakEmbedding)
        XCTAssertEqual(probe.sessionDestructions, 1)
    }

    func testCopiedBindingRetainsOpenEmbeddingUntilExplicitClose() throws {
        let probe = EmbeddingOperationsProbe()
        var embedding: SelectionEmbedding? = try SelectionEmbedding(
            path: URL(fileURLWithPath: "/fixture/selection.ron"),
            admittedOperations: probe.operations()
        )
        weak var weakEmbedding = embedding
        defer { weakEmbedding?.close() }
        var model: SelectionPickerModel? = embedding!.makePickerModel()
        weak var weakModel = model
        var original: Binding<FamilyActivitySelection>? = model!.binding
        var copy = original

        // The Binding must retain the owning object, not merely its session address.
        embedding = nil
        model = nil
        original = nil
        XCTAssertNotNil(weakEmbedding)
        XCTAssertNotNil(weakModel)
        XCTAssertNotNil(probe.session)
        XCTAssertEqual(probe.releases, 0)
        XCTAssertEqual(probe.sessionDestructions, 0)

        copy!.wrappedValue = FamilyActivitySelection()
        XCTAssertEqual(probe.commits, 1)
        XCTAssertEqual(probe.candidateReleaseCalls, 1)
        XCTAssertEqual(probe.invalidSessionUses, 0)
        XCTAssertFalse(weakModel!.recoveryRequired)

        weakEmbedding?.close()
        XCTAssertEqual(probe.releases, 1)
        XCTAssertEqual(probe.sessionDestructions, 1)
        XCTAssertNil(probe.session)
        copy = nil
        XCTAssertNil(weakModel)
        XCTAssertNil(weakEmbedding)
    }

    func testOpenFailureNeverReleasesAnUnopenedFixtureSession() {
        let probe = EmbeddingOperationsProbe()
        probe.openFails = true
        XCTAssertThrowsError(try SelectionEmbedding(
            path: URL(fileURLWithPath: "/fixture/selection.ron"),
            admittedOperations: probe.operations()
        ))
        XCTAssertEqual(probe.opens, 1)
        XCTAssertEqual(probe.restores, 0)
        XCTAssertEqual(probe.commits, 0)
        XCTAssertEqual(probe.releases, 0)
        XCTAssertEqual(probe.sessionDestructions, 0)
        XCTAssertNil(probe.session)
    }

    func testFailedEmbeddingCommitBlocksAllCopiedBindings() throws {
        let probe = EmbeddingOperationsProbe()
        probe.commitSucceeds = false
        let embedding = try SelectionEmbedding(
            path: URL(fileURLWithPath: "/fixture/selection.ron"),
            admittedOperations: probe.operations()
        )
        defer { embedding.close() }
        let model = embedding.makePickerModel()
        let initial = model.selection
        let proposed = FamilyActivitySelection(
            includeEntireCategory: !initial.includeEntireCategory
        )
        let original = model.binding
        let copy = original
        XCTAssertNotEqual(proposed.includeEntireCategory, initial.includeEntireCategory)

        original.wrappedValue = proposed
        XCTAssertTrue(model.recoveryRequired)
        XCTAssertEqual(probe.commits, 1)
        XCTAssertEqual(probe.candidateReleaseCalls, 1)

        XCTAssertEqual(model.selection, initial)
        XCTAssertEqual(original.wrappedValue.includeEntireCategory, initial.includeEntireCategory)

        copy.wrappedValue = proposed
        XCTAssertEqual(model.selection.includeEntireCategory, initial.includeEntireCategory)
        XCTAssertEqual(copy.wrappedValue.includeEntireCategory, initial.includeEntireCategory)
        XCTAssertEqual(probe.commits, 1)
        XCTAssertEqual(probe.candidateReleaseCalls, 1)
        XCTAssertEqual(probe.releases, 0)

        embedding.close()
        XCTAssertEqual(probe.releases, 1)
        XCTAssertEqual(probe.sessionDestructions, 1)
        XCTAssertEqual(probe.invalidSessionUses, 0)
    }

    func testEmbeddingRestoreFailuresReleaseTheOpenedFixtureSession() {
        for mode in [EmbeddingOperationsProbe.RestoreMode.fail, .invalid] {
            let probe = EmbeddingOperationsProbe()
            probe.restoreMode = mode
            XCTAssertThrowsError(try SelectionEmbedding(
                path: URL(fileURLWithPath: "/fixture/selection.ron"),
                admittedOperations: probe.operations()
            ))
            XCTAssertEqual(probe.opens, 1)
            XCTAssertEqual(probe.restores, 1)
            XCTAssertEqual(probe.releases, 1)
            XCTAssertEqual(probe.sessionDestructions, 1)
            XCTAssertEqual(probe.commits, 0)
            XCTAssertNil(probe.session)
        }
    }

    func testEmbeddingRestoresCodecBytesBeforeCreatingItsPickerModel() throws {
        let probe = EmbeddingOperationsProbe()
        // A default value would let an ignored restore pass this test. Toggle
        // only the public flag; no application/category token is fabricated.
        let saved = FamilyActivitySelection(
            includeEntireCategory: !FamilyActivitySelection().includeEntireCategory
        )
        probe.savedData = try SelectionCodec.encode(saved)
        let embedding = try SelectionEmbedding(
            path: URL(fileURLWithPath: "/fixture/selection.ron"),
            admittedOperations: probe.operations()
        )
        defer { embedding.close() }
        let model = embedding.makePickerModel()
        XCTAssertEqual(model.selection, saved)
        XCTAssertEqual(model.selection.includeEntireCategory, saved.includeEntireCategory)
        XCTAssertNotEqual(model.selection.includeEntireCategory,
                          FamilyActivitySelection().includeEntireCategory)
        XCTAssertEqual(probe.opens, 1)
        XCTAssertEqual(probe.restores, 1)
        XCTAssertEqual(probe.commits, 0)
        embedding.close()
        XCTAssertEqual(probe.releases, 1)
        XCTAssertEqual(probe.sessionDestructions, 1)
    }
}


// The injected session is a Swift test fixture, never a Rust session or a token.
// Candidate handles still use the production Swift Selection bridge. Empty
// selections here test ownership only; these cases do not select an application.
@available(iOS 16.0, *)
private final class EmbeddingOperationsProbe {
    enum RestoreMode { case normal, fail, invalid }
    final class Session {
        let destroyed: () -> Void
        init(destroyed: @escaping () -> Void) { self.destroyed = destroyed }
        deinit { destroyed() }
    }

    weak var session: Session?
    private var ownedSession: UnsafeMutableRawPointer?
    var opens = 0
    var restores = 0
    var commits = 0
    var releases = 0
    var candidateReleaseCalls = 0
    var sessionDestructions = 0
    var invalidSessionUses = 0
    var openFails = false
    var commitSucceeds = true
    var savedData: Data?
    var restoreMode = RestoreMode.normal

    @MainActor
    func operations() -> SelectionEmbedding.Operations {
        SelectionEmbedding.Operations(
            open: { [self] _ in
                XCTAssertNil(ownedSession)
                opens += 1
                if openFails { throw CocoaError(.fileReadUnknown) }
                let opened = Session { [weak self] in
                    self?.sessionDestructions += 1
                }
                session = opened
                let pointer = Unmanaged.passRetained(opened).toOpaque()
                ownedSession = pointer
                return pointer
            },
            restored: { [self] pointer in
                restores += 1
                XCTAssertEqual(pointer, ownedSession)
                switch restoreMode {
                case .normal: return savedData
                case .fail: throw CocoaError(.fileReadUnknown)
                case .invalid: return Data([0xff])
                }
            },
            commit: { [self] pointer, candidate in
                commits += 1
                defer {
                    selectionRelease(candidate)
                    candidateReleaseCalls += 1
                }
                guard pointer == ownedSession else {
                    invalidSessionUses += 1
                    XCTFail("commit used an unowned or released fixture session")
                    return false
                }
                return commitSucceeds
            },
            release: { [self] pointer in
                releases += 1
                guard pointer == ownedSession else {
                    invalidSessionUses += 1
                    XCTFail("fixture session was released more than once")
                    return
                }
                ownedSession = nil
                Unmanaged<Session>.fromOpaque(pointer).release()
            }
        )
    }

    deinit {
        // Failed assertions must not leave this test fixture's +1 behind.
        if let pointer = ownedSession {
            Unmanaged<Session>.fromOpaque(pointer).release()
        }
    }
}
