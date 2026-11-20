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
    $0.feedback = error.localizedDescription
  }
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
