import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct GitHubGraphSheet: View {
  let store: StoreOf<GitHubConnector>
  @State private var root = ""
  @State private var symbol = ""

  var body: some View {
    GitHubSheet(
      title: "Code Graph", message: store.feedback, busy: store.busy && store.graphState != nil
    ) {
      Group {
        if store.graphState == nil && store.busy {
          LoadingStateView("Loading code graph…")
        } else {
          graphContents
        }
      }
      .frame(height: 470)
    } buttons: {
      Button("Back") { store.send(.dismissPanel) }
        .disabled(store.busy)
        .accessibilityIdentifier("github-graph-back")
      Spacer()
      if let graph = store.graphState, graph.access == .available {
        Button("Refresh Source") { store.send(.refreshCode(graph.repositoryId)) }
          .disabled(store.busy)
      }
      Button("Reload", systemImage: "arrow.clockwise") {
        store.send(.lookupGraph(root: store.graphRoot, symbol: store.graphSymbol))
      }
      .disabled(store.busy || store.graphState == nil)
    }
  }

  private var graphContents: some View {
    Form {
      if let graph = store.graphState {
        LabeledContent("Branch", value: graph.branch)
        if let snapshot = graph.snapshot {
          LabeledContent(
            graph.access == .retained ? "Retained commit" : "Indexed commit",
            value: String(snapshot.revision.prefix(10)))
          switch snapshot.coverage {
          case .published: lookup(snapshot)
          case .noConventionalRustRoots:
            Text("No src/lib.rs or src/main.rs found in this snapshot.")
          case .limit: Text("This snapshot exceeds the graph projection limit.")
          }
        } else {
          Text("No graph has been published for this branch.")
        }
      }
    }
    .formStyle(.grouped)
  }

  @ViewBuilder
  private func lookup(_ snapshot: Components.Schemas.GitHubGraphSnapshot) -> some View {
    let selected = snapshot.roots.first { $0.path == root } ?? snapshot.roots.first
    if let selected {
      Section {
        Picker("Rust source root", selection: Binding(get: { selected.path }, set: { root = $0 })) {
          ForEach(snapshot.roots, id: \.path) { Text($0.path).tag($0.path) }
        }
        Text(
          "\(selected.reachableFiles) reachable files · \(selected.parseErrors) parse errors · \(selected.limitedFiles) limited"
        )
        .font(.caption).foregroundStyle(.secondary)
        TextField("Symbol", text: $symbol, prompt: Text("Name or crate::path::name"))
          .accessibilityIdentifier("github-graph-symbol")
        Button("Find Symbol") {
          store.send(
            .lookupGraph(root: selected.path, symbol: symbol.trimmingCharacters(in: .whitespaces)))
        }
        .disabled(symbol.trimmingCharacters(in: .whitespaces).isEmpty || store.busy)
        .accessibilityIdentifier("github-graph-find")
      }
    }
    if let searched = store.graphSymbol, let searchedRoot = store.graphRoot {
      Section("Results for \(searched) in \(searchedRoot)") {
        ForEach(snapshot.hits.indices, id: \.self) { index in
          let hit = snapshot.hits[index]
          VStack(alignment: .leading, spacing: 4) {
            Text(hit.qualifiedName ?? hit.name).font(.headline)
            Text(
              hit.role == .definition
                ? "Definition" : "\(hit.role.rawValue.capitalized) · \(hit.resolution.rawValue)"
            )
            .accessibilityIdentifier("github-graph-\(hit.role.rawValue)-\(hit.resolution.rawValue)")
            source(hit.site)
            if let target = hit.target { source(target, label: "Definition") }
          }
        }
        if snapshot.hits.isEmpty { Text("No matching symbol in this source root.") }
        if snapshot.truncated { Text("Showing the first 50 occurrences.") }
      }
    }
  }

  @ViewBuilder
  private func source(_ location: Components.Schemas.SourceLocation, label: String? = nil)
    -> some View
  {
    let lines =
      location.startLine == location.endLine
      ? "L\(location.startLine)" : "L\(location.startLine)-L\(location.endLine)"
    if let url = URL(string: "\(location.fileUrl)#\(lines)") {
      Link(
        [label, "\(location.path)#\(lines)"].compactMap { $0 }.joined(separator: " · "),
        destination: url
      )
      .font(.callout)
    }
  }
}
