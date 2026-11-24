import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

private let repository = HubCatalog.Repository(
  repository: "fixture/catalog-qwen", revision: String(repeating: "a", count: 40),
  url: "https://huggingface.co/fixture/catalog-qwen")
private let variant = HubCatalog.Variant(
  file: "fixture-Q4_K_M.gguf", quantization: .Q4_K_M, bytes: 42,
  sha256: String(repeating: "b", count: 64))

@MainActor
@Test
func hubCatalogKeepsThePinnedSelectionAndReturnsToTheLibraryAfterAcceptance() async {
  let clock = TestClock()
  let retained = Components.Schemas.ModelDownload(
    profile: "new-model", state: .installing, downloadedBytes: 15, totalBytes: 42)
  let store = observedLibrary(catalog: retained, clock: clock)

  await store.send(.observe) { $0.isObserving = true }
  await store.receive(\.catalogLoaded) { $0.profiles = [retained] }
  await store.send(.openDiscovery) { $0.showingDiscovery = true }
  await store.send(.hub(.search("fixture"))) {
    $0.hub.query = "fixture"
    $0.hub.operation = .searching
  }
  await store.receive(\.hub.repositoriesLoaded) {
    $0.hub.operation = nil
    $0.hub.repositories = [repository]
  }
  await store.send(.hub(.openRepository(repository))) {
    $0.hub.repository = repository
    $0.hub.operation = .loadingVariants
  }
  await store.receive(\.hub.variantsLoaded) {
    $0.hub.operation = nil
    $0.hub.variants = [variant]
  }
  await store.send(.hub(.download))
  await store.send(.hub(.selectVariant(variant))) { $0.hub.selectedVariant = variant }
  await clock.advance(by: .seconds(1))
  await store.receive(\.catalogLoaded)
  #expect(store.state.hub.selectedVariant == variant)
  await store.send(.hub(.download)) { $0.hub.operation = .addingDownload }
  await store.receive(\.hub.downloadAccepted) {
    $0.hub = HubCatalog.State()
    $0.showingDiscovery = false
  }
  await store.receive(\.catalogLoaded)
  await store.send(.stopObserving) {
    $0.isObserving = false
    $0.profiles = nil
  }
}

@MainActor
private func observedLibrary(
  catalog retained: Components.Schemas.ModelDownload, clock: TestClock<Duration>
)
  -> TestStore<ModelLibrary.State, ModelLibrary.Action>
{
  TestStore(initialState: ModelLibrary.State()) {
    ModelLibrary(
      client: .init(
        catalog: { [retained] }, install: { _ in },
        hub: .init(
          search: { query, kind in
            #expect(kind == .generation)
            #expect(query == "fixture")
            return [repository]
          },
          variants: { selection, kind in
            #expect(kind == .generation)
            #expect(selection == repository)
            return [variant]
          },
          install: { selection, file, kind in
            #expect(kind == .generation)
            #expect(selection == repository && file == variant)
            return retained.profile
          })))
  } withDependencies: {
    $0.continuousClock = clock
  }

}

@MainActor
@Test
func hubCatalogRecoversFromProviderFailureAndCanLeaveVariantSelection() async {
  let unavailable = BackendConnectionError.helperExited
  let store = TestStore(initialState: HubCatalog.State()) {
    HubCatalog(
      client: .init(
        search: { _, _ in [repository] },
        variants: { _, _ in [variant] },
        install: { _, _, _ in throw unavailable }))
  }
  await store.send(.openRepository(repository))
  await store.send(.selectVariant(variant))
  await store.send(.search("")) { $0.operation = .searching }
  await store.receive(\.repositoriesLoaded) {
    $0.operation = nil
    $0.repositories = [repository]
  }
  await store.send(.openRepository(repository)) {
    $0.repository = repository
    $0.operation = .loadingVariants
  }
  await store.receive(\.variantsLoaded) {
    $0.operation = nil
    $0.variants = [variant]
  }
  await store.send(.selectVariant(variant)) { $0.selectedVariant = variant }
  await store.send(.download) { $0.operation = .addingDownload }
  await store.receive(\.failed) {
    $0.operation = nil
    $0.feedback = unavailable.localizedDescription
  }
  #expect(store.state.selectedVariant == variant)
  await store.send(.back) {
    $0.repository = nil
    $0.variants = nil
    $0.selectedVariant = nil
    $0.feedback = nil
  }
  await store.send(.close) { $0.repositories = nil }
}

@MainActor
@Test
func embeddingDiscoveryKeepsRoleThroughSearchSelectionAndDownload() async {
  let unverified = HubCatalog.Variant(file: "unverified.gguf", quantization: .Q4_K_M, bytes: 42)
  let store = TestStore(initialState: HubCatalog.State()) {
    HubCatalog(
      client: .init(
        search: { query, kind in
          #expect(query == "embedding" && kind == .embedding)
          return [repository]
        },
        variants: { selected, kind in
          #expect(selected == repository && kind == .embedding)
          return [variant, unverified]
        },
        install: { selected, file, kind in
          #expect(selected == repository && file == variant && kind == .embedding)
          return "embedding-profile"
        }))
  }
  await store.send(.selectKind(.embedding)) { $0.kind = .embedding }
  await openEmbeddingVariants(in: store, unverified: unverified)
  await store.send(.selectVariant(unverified))
  await store.send(.selectVariant(variant)) { $0.selectedVariant = variant }
  await store.send(.selectKind(.generation)) { $0 = HubCatalog.State() }
  await store.send(.selectKind(.embedding)) { $0.kind = .embedding }
  await openEmbeddingVariants(in: store, unverified: unverified)
  await store.send(.selectVariant(variant)) { $0.selectedVariant = variant }
  await store.send(.download) { $0.operation = .addingDownload }
  await store.receive(\.downloadAccepted) { $0 = HubCatalog.State() }
}

@MainActor
private func openEmbeddingVariants(
  in store: TestStore<HubCatalog.State, HubCatalog.Action>, unverified: HubCatalog.Variant
) async {
  await store.send(.search("embedding")) {
    $0.query = "embedding"
    $0.operation = .searching
  }
  await store.receive(\.repositoriesLoaded) {
    $0.operation = nil
    $0.repositories = [repository]
  }
  await store.send(.openRepository(repository)) {
    $0.repository = repository
    $0.operation = .loadingVariants
  }
  await store.receive(\.variantsLoaded) {
    $0.operation = nil
    $0.variants = [variant, unverified]
  }
}
