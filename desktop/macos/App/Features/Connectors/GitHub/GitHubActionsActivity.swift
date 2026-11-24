import LocalAPI
import SwiftUI

struct GitHubActionsActivity: View {
  @Environment(\.locale) private var locale
  private enum Filter: Hashable {
    case all
    case workflow(Int64)
  }

  let snapshot: Components.Schemas.GitHubActions
  @State private var filter: Filter = .all

  private var visibleRuns: [Components.Schemas.GitHubActionRun] {
    snapshot.runs.filter { filter == .all || filter == .workflow($0.workflowId) }
      .sorted(using: [
        KeyPathComparator(\.createdAt, order: .reverse),
        KeyPathComparator(\.id, order: .reverse),
        KeyPathComparator(\.attempt, order: .reverse),
      ])
  }

  var body: some View {
    HSplitView {
      List {
        Section("Workflows") {
          workflowButton(locale.localized("All Workflows"), icon: "square.stack", selection: .all)
          ForEach(snapshot.workflows, id: \.id) { workflow in
            workflowButton(
              workflow.name, icon: "arrow.triangle.branch", selection: .workflow(workflow.id)
            )
            .help(workflow.path)
            .accessibilityIdentifier("github-action-workflow-\(workflow.id)")
          }
        }
      }
      .listStyle(.sidebar)
      .frame(minWidth: 175, idealWidth: 205, maxWidth: 225)

      let runs = visibleRuns
      VStack(alignment: .leading, spacing: 0) {
        HStack(alignment: .firstTextBaseline) {
          Text("Recent Runs").font(.headline).accessibilityAddTraits(.isHeader)
          Spacer()
          Text("\(runs.count) observed")
            .font(.caption).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        Divider()
        if runs.isEmpty {
          ContentUnavailableView(
            "No Runs", systemImage: "play.rectangle",
            description: Text("There are no observed runs for this selection.")
          )
          .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          List(runs, id: \.identity) { run in
            GitHubActionsRunRow(
              run: run, title: workflowName(run.workflowId), repository: snapshot.repository)
          }
          .listStyle(.inset)
        }
      }
      .frame(minWidth: 460)
    }
  }

  private func workflowName(_ id: Int64) -> String {
    snapshot.workflows.first { $0.id == id }?.name ?? locale.localized("Workflow #\(id)")
  }

  private func workflowButton(_ title: String, icon: String, selection: Filter) -> some View {
    Button {
      filter = selection
    } label: {
      Label(title, systemImage: icon)
        .lineLimit(1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(filter == selection ? .isSelected : [])
    .listRowBackground(filter == selection ? Color.accentColor.opacity(0.18) : Color.clear)
  }
}

private struct GitHubActionsRunRow: View {
  @Environment(\.locale) private var locale
  let run: Components.Schemas.GitHubActionRun
  let title: String
  let repository: String

  private var result: String { run.conclusion ?? run.status }

  private var resultTitle: String {
    switch result {
    case "success": locale.localized("Success")
    case "failure": locale.localized("Failure")
    case "cancelled": locale.localized("Cancelled")
    case "queued": locale.localized("Queued")
    case "in_progress": locale.localized("In Progress")
    case "completed": locale.localized("Completed")
    case "skipped": locale.localized("Skipped")
    case "timed_out": locale.localized("Timed Out")
    default: result.replacingOccurrences(of: "_", with: " ").capitalized
    }
  }

  private var icon: (name: String, color: Color) {
    switch result {
    case "success": ("checkmark.circle.fill", .green)
    case "failure", "timed_out", "action_required": ("exclamationmark.circle.fill", .red)
    case "queued", "in_progress", "waiting", "pending": ("clock.fill", .orange)
    default: ("minus.circle", .secondary)
    }
  }

  var body: some View {
    HStack(spacing: 12) {
      Image(systemName: icon.name)
        .foregroundStyle(icon.color)
        .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 4) {
        Text(verbatim: title).font(.body.weight(.medium)).lineLimit(1)
        Text(
          "Run \(run.id) · attempt \(run.attempt) · \(run.event) · \(run.headSha.prefix(10))"
        )
        .font(.caption.monospacedDigit())
        .foregroundStyle(.secondary)
        .lineLimit(1)
      }
      .accessibilityElement(children: .ignore)
      .accessibilityLabel(
        "\(title) · Run \(run.id) · attempt \(run.attempt) · \(result) · \(run.headSha.prefix(10))"
      )
      .accessibilityIdentifier("github-action-run-\(run.id)-\(run.attempt)")
      Spacer(minLength: 8)
      VStack(alignment: .trailing, spacing: 4) {
        Text(resultTitle)
          .font(.callout)
        Text(run.createdAt, format: .dateTime.month(.abbreviated).day().hour().minute())
          .font(.caption.monospacedDigit())
          .foregroundStyle(.secondary)
      }
      if let url = runURL {
        Link(destination: url) { Image(systemName: "arrow.up.right.square") }
          .help("Open this attempt on GitHub")
          .accessibilityLabel("Open run \(run.id), attempt \(run.attempt) on GitHub")
          .accessibilityIdentifier("github-action-link-\(run.id)-\(run.attempt)")
      }
    }
    .padding(.vertical, 5)
  }

  private var runURL: URL? {
    var components = URLComponents()
    components.scheme = "https"
    components.host = "github.com"
    components.path = "/\(repository)/actions/runs/\(run.id)/attempts/\(run.attempt)"
    return components.url
  }
}

extension Components.Schemas.GitHubActionRun {
  fileprivate var identity: String { "\(id)-\(attempt)" }
}
