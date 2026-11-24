import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func graphNavigationKeepsRootAndSymbolBoundToThePublishedBranch() async {
  let graph = Components.Schemas.GitHubGraph(repositoryId: 41, branch: "main", access: .retained)
  let store = TestStore(initialState: GitHubConnector.State()) {
    GitHubConnector { command in
      guard case .graph(let id, let branch, let root, let symbol) = command else {
        throw BackendConnectionError.helperExited
      }
      #expect(id == 41 && branch == "main")
      return .graph(graph, root: root, symbol: symbol)
    }
  }
  await store.send(.openGraph(id: 41, branch: "main")) {
    $0.panel = .graph
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.graphState = graph
  }
  await store.send(.lookupGraph(root: "src/lib.rs", symbol: "example")) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.graphRoot = "src/lib.rs"
    $0.graphSymbol = "example"
  }
  await store.send(.dismissPanel) { $0.panel = .editOptions }
}
