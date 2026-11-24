import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func modelLibraryRetainsPendingUntilTheBackendConfirmsProgress() async {
  let clock = TestClock()
  let profile = "fixture-model"
  let idle = Components.Schemas.ModelDownload(
    profile: profile, state: .notInstalled, downloadedBytes: 0, totalBytes: 100)
  var current = idle
  current.state = .installing
  current.downloadedBytes = 40
  let installing = current
  current.state = .installed
  current.downloadedBytes = 100
  let installed = current
  let fixture = TransferFixture(catalogs: [[idle], [installing], [installed]])
  let store = TestStore(initialState: ModelLibrary.State()) {
    ModelLibrary(
      client: .init(
        catalog: { await fixture.nextCatalog() },
        install: {
          #expect($0 == profile)
          await fixture.submitted()
        }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observe) { $0.isObserving = true }
  await store.receive(\.catalogLoaded) { $0.profiles = [idle] }
  await store.send(.install("unknown"))
  await store.send(.install(profile)) { $0.pendingProfile = profile }
  await store.receive(\.installationAccepted) { $0.acceptedProfile = profile }
  await store.receive(\.catalogLoaded) {
    $0.pendingProfile = nil
    $0.acceptedProfile = nil
    $0.profiles = [installing]
  }
  await clock.advance(by: .seconds(1))
  await store.receive(\.catalogLoaded) { $0.profiles = [installed] }
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.profiles = nil
  }
  #expect(await fixture.submissions == 1)
}

@MainActor
@Test
func modelLibraryPresentsCatalogAndInstallationFailuresSeparately() async {
  let clock = TestClock()
  let error = BackendConnectionError.helperExited
  let model = Components.Schemas.ModelDownload(
    profile: "fixture-model", state: .failed, downloadedBytes: 20, totalBytes: 100)
  var initial = ModelLibrary.State()
  initial.profiles = [model]
  let store = TestStore(initialState: initial) {
    ModelLibrary(client: .init(catalog: { throw error }, install: { _ in throw error }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observe) { $0.isObserving = true }
  await store.receive(\.catalogFailed) { $0.catalogFeedback = error.localizedDescription }
  await store.send(.install(model.profile)) { $0.pendingProfile = model.profile }
  await store.receive(\.installationRejected) {
    $0.pendingProfile = nil
    $0.feedbackProfile = model.profile
    $0.feedback = error.localizedDescription
  }
  await store.send(.catalogLoaded([model])) { $0.catalogFeedback = nil }
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.profiles = nil
  }
}

private actor TransferFixture {
  var submissions = 0
  private var catalogs: [[Components.Schemas.ModelDownload]]

  init(catalogs: [[Components.Schemas.ModelDownload]]) {
    self.catalogs = catalogs
  }

  func nextCatalog() -> [Components.Schemas.ModelDownload] { catalogs.removeFirst() }
  func submitted() { submissions += 1 }
}

@MainActor
@Test
func modelLibraryOnlyDeletesAnIdleModelAndReloadsTheCatalog() async {
  let clock = TestClock()
  let available = Components.Schemas.ModelDownload(
    profile: "idle", state: .installed, downloadedBytes: 100, totalBytes: 100)
  let blocked = Components.Schemas.ModelDownload(
    profile: "active", deletionBlock: "Switch models first.", state: .installed,
    downloadedBytes: 100, totalBytes: 100)
  let removed = Components.Schemas.ModelDownload(
    profile: "idle", state: .notInstalled, downloadedBytes: 0, totalBytes: 100)
  let fixture = TransferFixture(catalogs: [[available, blocked], [removed, blocked]])
  let store = TestStore(initialState: ModelLibrary.State()) {
    ModelLibrary(
      client: .init(
        catalog: { await fixture.nextCatalog() },
        install: { _ in throw BackendConnectionError.helperExited },
        delete: { profile in
          #expect(profile == available.profile)
          await fixture.submitted()
        }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observe) { $0.isObserving = true }
  await store.receive(\.catalogLoaded) { $0.profiles = [available, blocked] }
  await store.send(.delete(blocked.profile))
  await store.send(.delete(available.profile)) { $0.pendingProfile = available.profile }
  await store.receive(\.deletionCompleted) { $0.acceptedProfile = available.profile }
  await store.receive(\.catalogLoaded) {
    $0.profiles = [removed, blocked]
    $0.pendingProfile = nil
    $0.acceptedProfile = nil
  }
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.profiles = nil
  }
  #expect(await fixture.submissions == 1)
}

@MainActor
@Test
func modelLibraryOnlyPausesActiveAndExplicitlyResumesStoppedTransfers() async {
  var initial = ModelLibrary.State()
  let active = Components.Schemas.ModelDownload(
    profile: "fixture", state: .installing, downloadedBytes: 5, totalBytes: 11)
  var paused = active
  paused.state = .paused
  initial.profiles = [active]
  let calls = LockIsolated<[String]>([])
  let store = TestStore(initialState: initial) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        pause: { profile in calls.withValue { $0.append("pause:" + profile) } },
        resume: { profile in calls.withValue { $0.append("resume:" + profile) } }))
  }
  await store.send(.resume("fixture"))
  await store.send(.pause("fixture")) { $0.pendingProfile = "fixture" }
  await store.receive(\.installationAccepted) { $0.pendingProfile = nil }
  await store.send(.catalogLoaded([paused])) { $0.profiles = [paused] }
  await store.send(.pause("fixture"))
  await store.send(.resume("fixture")) { $0.pendingProfile = "fixture" }
  await store.receive(\.installationAccepted) { $0.pendingProfile = nil }
  #expect(calls.value == ["pause:fixture", "resume:fixture"])
}

@MainActor
@Test
func modelLibraryDiscardsOnlyStoppedPartialDownloads() async {
  let calls = LockIsolated<[String]>([])
  var initial = ModelLibrary.State()
  var model = Components.Schemas.ModelDownload(
    profile: "fixture", state: .installing, downloadedBytes: 5, totalBytes: 11)
  initial.profiles = [model]
  let store = TestStore(initialState: initial) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        discard: { profile in
          calls.withValue { $0.append(profile) }
        }))
  }
  await store.send(.discardPartial("fixture"))
  model.state = .paused
  await store.send(.catalogLoaded([model])) { $0.profiles = [model] }
  await store.send(.discardPartial("fixture")) { $0.pendingProfile = "fixture" }
  await store.receive(\.installationAccepted) { $0.pendingProfile = nil }
  model.state = .interrupted
  await store.send(.catalogLoaded([model])) { $0.profiles = [model] }
  await store.send(.discardPartial("fixture")) { $0.pendingProfile = "fixture" }
  await store.receive(\.installationAccepted) { $0.pendingProfile = nil }
  #expect(calls.value == ["fixture", "fixture"])
}

@MainActor
@Test
func runtimeInstallationUsesSelectedArchiveAndReportsBackendOutcome() async {
  let failed = Components.Schemas.RuntimeStatus(state: .failed, error: "Verification failed")
  let ready = Components.Schemas.RuntimeStatus(state: .ready)
  var initial = ModelLibrary.State()
  initial.runtime = failed
  let paths = LockIsolated<[String]>([])
  let store = TestStore(initialState: initial) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        installRuntime: { path in
          paths.withValue { $0.append(path) }
          return ready
        }))
  }
  await store.send(.installRuntime("/tmp/runtime.zip")) {
    $0.runtimeRequestPending = true
  }
  await store.receive(\.runtimeInstallCompleted) {
    $0.runtimeRequestPending = false
    $0.runtime = ready
  }
  await store.send(.installRuntime("/tmp/another.zip"))
  #expect(paths.value == ["/tmp/runtime.zip"])
}

@MainActor
@Test
func runtimeObservationRecoversAndContinuesAfterInstallRequestFails() async {
  let clock = TestClock()
  let calls = LockIsolated(0)
  let failure = BackendConnectionError.helperExited
  let available = Components.Schemas.RuntimeStatus(state: .availableForInstall, archiveBytes: 128)
  let store = TestStore(initialState: ModelLibrary.State()) {
    ModelLibrary(
      client: .init(
        catalog: {
          try await clock.sleep(for: .seconds(60))
          return []
        },
        install: { _ in },
        runtimeStatus: {
          let attempt = calls.withValue { count in
            count += 1
            return count
          }
          if attempt == 1 { throw failure }
          return available
        },
        installRuntime: { _ in throw failure }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observe) { $0.isObserving = true }
  await store.receive(\.runtimeFailed) { $0.runtimeStatusFeedback = failure.localizedDescription }
  await clock.advance(by: .seconds(1))
  await store.receive(\.runtimeLoaded) {
    $0.runtime = available
    $0.runtimeStatusFeedback = nil
  }
  await store.send(.installRuntime("/tmp/runtime.zip")) { $0.runtimeRequestPending = true }
  await store.receive(\.runtimeInstallFailed) {
    $0.runtimeRequestPending = false
    $0.runtimeFeedback = failure.localizedDescription
  }
  await store.receive(\.runtimeLoaded)
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.runtime = nil
  }
  await clock.advance(by: .seconds(1))
  #expect(calls.value == 3)
}

@MainActor
@Test
func runtimeDownloadReportsProgressAndPollsUntilReady() async {
  let clock = TestClock()
  let available = Components.Schemas.RuntimeStatus(state: .availableForInstall, canDownload: true)
  let installing = Components.Schemas.RuntimeStatus(state: .installing, canDownload: true)
  let ready = Components.Schemas.RuntimeStatus(state: .ready, canDownload: true)
  let statuses = LockIsolated([ready])
  let calls = LockIsolated(0)
  var initial = ModelLibrary.State()
  initial.runtime = available
  initial.isObserving = true
  let store = TestStore(initialState: initial) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        runtimeStatus: { statuses.withValue { $0.removeFirst() } },
        downloadRuntime: {
          calls.withValue { $0 += 1 }
          return installing
        }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.downloadRuntime) { $0.runtimeRequestPending = true }
  await store.receive(\.runtimeInstallCompleted) {
    $0.runtimeRequestPending = false
    $0.runtime = installing
  }
  await store.receive(\.runtimeLoaded) { $0.runtime = ready }
  await store.send(.downloadRuntime)
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.runtime = nil
  }
  #expect(calls.value == 1)
}

@MainActor
@Test
func modelLibraryLoadsAndClearsAdvisoryStoragePlan() async {
  let clock = TestClock()
  let plan = Components.Schemas.InferenceStoragePlan(models: [])
  let store = TestStore(initialState: ModelLibrary.State()) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in }, storagePlan: { plan }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observe) { $0.isObserving = true }
  await store.receive(\.catalogLoaded) { $0.profiles = [] }
  await store.receive(\.storageLoaded) { $0.storagePlan = plan }
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.profiles = nil
    $0.storagePlan = nil
  }
}

@MainActor
@Test
func modelRemovalConfirmsTheExactPlannedIdentity() async {
  let model = Components.Schemas.ModelDownload(
    profile: "fixture", state: .installed, downloadedBytes: 42, totalBytes: 42)
  let plan = Components.Schemas.ModelRemovalPlan(
    profile: "fixture", bytes: 42, identity: "identity-1")
  let calls = LockIsolated<[String]>([])
  var initial = ModelLibrary.State()
  initial.profiles = [model]
  let store = TestStore(initialState: initial) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        removalPlan: { _ in plan },
        deleteConfirmed: { profile, identity in
          calls.withValue { $0.append(profile + ":" + identity) }
        }))
  }
  await store.send(.prepareRemoval("fixture")) { $0.pendingProfile = "fixture" }
  await store.receive(\.removalPlanLoaded) {
    $0.pendingProfile = nil
    $0.removalPlan = plan
  }
  await store.send(.confirmRemoval) {
    $0.removalPlan = nil
    $0.pendingProfile = "fixture"
  }
  await store.receive(\.deletionCompleted) { $0.pendingProfile = nil }
  #expect(calls.value == ["fixture:identity-1"])
}

@MainActor
@Test
func indexDeletionReportsLogicalBytesAndRetainsBackendState() async {
  let current = Components.Schemas.EmbeddingIndexState(
    profile: "embedding", enabled: true, epoch: 1)
  let disabled = Components.Schemas.EmbeddingIndexState(
    profile: "embedding", enabled: false, epoch: 2)
  let deletion = Components.Schemas.EmbeddingIndexDeletion(
    value1: disabled, value2: .init(removedVectors: 2, removedBytes: 1024))
  var initial = ModelLibrary.State()
  initial.index = current
  let store = TestStore(initialState: initial) {
    ModelLibrary(client: .init(catalog: { [] }, install: { _ in }, deleteIndex: { deletion }))
  }
  await store.send(.deleteIndex) { $0.indexPending = true }
  await store.receive(\.indexDeleted) {
    $0.indexPending = false
    $0.index = disabled
    $0.indexRemoval = deletion
  }
}

@MainActor
@Test
func changedModelRequiresNewRemovalConfirmation() async {
  let model = Components.Schemas.ModelDownload(
    profile: "fixture", state: .installed, downloadedBytes: 42, totalBytes: 42)
  let old = Components.Schemas.ModelRemovalPlan(profile: "fixture", bytes: 42, identity: "old")
  let fresh = Components.Schemas.ModelRemovalPlan(profile: "fixture", bytes: 43, identity: "new")
  var initial = ModelLibrary.State()
  initial.profiles = [model]
  initial.removalPlan = old
  let store = TestStore(initialState: initial) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in }, removalPlan: { _ in fresh },
        deleteConfirmed: { _, _ in throw ModelRemovalConflict() }))
  }
  await store.send(.confirmRemoval) {
    $0.removalPlan = nil
    $0.pendingProfile = "fixture"
  }
  await store.receive(\.removalConflict) {
    $0.pendingProfile = nil
    $0.feedback = "The model changed. Review the updated removal plan."
  }
  await store.receive(\.prepareRemoval) { $0.pendingProfile = "fixture" }
  await store.receive(\.removalPlanLoaded) {
    $0.pendingProfile = nil
    $0.removalPlan = fresh
  }
}

@MainActor
@Test
func indexUpdateFailureCanBeRetriedAndStalePollsAreIgnored() async {
  let error = BackendConnectionError.helperExited
  let enabled = Components.Schemas.EmbeddingIndexState(
    profile: "embedding", enabled: true, epoch: 1)
  let disabled = Components.Schemas.EmbeddingIndexState(
    profile: "embedding", enabled: false, epoch: 2)
  let calls = LockIsolated(0)
  var initial = ModelLibrary.State()
  initial.index = enabled
  let store = TestStore(initialState: initial) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        setIndexEnabled: { value in
          #expect(!value)
          let attempt = calls.withValue {
            $0 += 1
            return $0
          }
          if attempt == 1 { throw error }
          return disabled
        }))
  }
  await store.send(.setIndexEnabled(true))
  await store.send(.setIndexEnabled(false)) { $0.indexPending = true }
  await store.receive(\.indexRequestFailed) {
    $0.indexPending = false
    $0.indexFeedback = error.localizedDescription
  }
  await store.send(.setIndexEnabled(false)) {
    $0.indexPending = true
    $0.indexFeedback = nil
  }
  await store.receive(\.indexUpdated) {
    $0.indexPending = false
    $0.index = disabled
  }
  #expect(calls.value == 2)
}

@MainActor
@Test
func maintenancePollingRecoversFromUnavailableStatus() async {
  let clock = TestClock()
  let error = BackendConnectionError.helperExited
  let index = Components.Schemas.EmbeddingIndexState(profile: "embedding", enabled: true, epoch: 1)
  let plan = Components.Schemas.InferenceStoragePlan(models: [])
  let indexCalls = LockIsolated(0)
  let storageCalls = LockIsolated(0)
  let store = TestStore(initialState: ModelLibrary.State()) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        indexStatus: {
          let attempt = indexCalls.withValue {
            $0 += 1
            return $0
          }
          if attempt == 1 { throw error }
          return index
        },
        storagePlan: {
          let attempt = storageCalls.withValue {
            $0 += 1
            return $0
          }
          if attempt == 1 { throw error }
          return plan
        }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observe) { $0.isObserving = true }
  await store.receive(\.catalogLoaded) { $0.profiles = [] }
  await store.receive(\.indexFailed) { $0.indexStatusFeedback = error.localizedDescription }
  await store.receive(\.storageFailed) { $0.storageFeedback = error.localizedDescription }
  await clock.advance(by: .seconds(1))
  await store.receive(\.catalogLoaded)
  await store.receive(\.indexLoaded) {
    $0.index = index
    $0.indexStatusFeedback = nil
  }
  await store.receive(\.storageLoaded) {
    $0.storagePlan = plan
    $0.storageFeedback = nil
  }
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.profiles = nil
    $0.index = nil
    $0.storagePlan = nil
  }
}

@MainActor
@Test
func cancellingModelRemovalDiscardsConfirmation() async {
  var initial = ModelLibrary.State()
  initial.removalPlan = .init(profile: "fixture", bytes: 10, identity: "old")
  initial.feedback = "The model changed. Review the updated removal plan."
  let store = TestStore(initialState: initial) {
    ModelLibrary(client: .init(catalog: { [] }, install: { _ in }))
  }
  await store.send(.cancelRemoval) {
    $0.removalPlan = nil
    $0.feedback = nil
  }
  await store.send(.confirmRemoval)
}

@MainActor
@Test
func indexIgnoresPollingWhileUpdateIsPending() async {
  let enabled = Components.Schemas.EmbeddingIndexState(
    profile: "embedding", enabled: true, epoch: 1)
  var initial = ModelLibrary.State()
  initial.index = enabled
  initial.indexPending = true
  let store = TestStore(initialState: initial) {
    ModelLibrary(client: .init(catalog: { [] }, install: { _ in }))
  }
  await store.send(.indexLoaded(enabled))
  await store.send(.indexFailed("stale"))
  await store.send(.setIndexEnabled(false))
}

@MainActor
@Test
func modelLibraryRoutesSetupReviewAndConfirmation() async {
  let selection = Components.Schemas.InferenceSetupSelection(
    embedding: false, runtimeSource: .hosted)
  let plan = Components.Schemas.InferenceSetupPlan(
    selection: selection, fingerprint: "parent-plan", peakAdditionalBytes: 20,
    maximumTransferBytes: 10, availableBytes: 100, canStart: true, runtimeRequired: true,
    answerInstalled: false)
  let status = Components.Schemas.InferenceSetupStatus(
    operationId: "00000000-0000-0000-0000-000000000001", phase: .runtime, answerReady: false)
  let calls = LockIsolated(0)
  let store = TestStore(initialState: ModelLibrary.State()) {
    ModelLibrary(
      client: .init(
        catalog: { [] }, install: { _ in },
        setup: .init(
          plan: { _ in plan },
          start: { _, fingerprint, _, _ in
            #expect(fingerprint == plan.fingerprint)
            calls.withValue { $0 += 1 }
            return status
          }, status: { nil })))
  } withDependencies: {
    $0.uuid = .incrementing
  }
  await store.send(.setup(.review(selection, nil))) { $0.setup.requesting = true }
  await store.receive(\.setup.planLoaded) {
    $0.setup.requesting = false
    $0.setup.plan = plan
  }
  #expect(calls.value == 0)
  await store.send(.setup(.confirm)) {
    $0.setup.plan = nil
    $0.setup.requesting = true
    $0.setup.starting = true
  }
  await store.receive(\.setup.statusLoaded) {
    $0.setup.requesting = false
    $0.setup.starting = false
    $0.setup.status = status
  }
  #expect(calls.value == 1)
}
