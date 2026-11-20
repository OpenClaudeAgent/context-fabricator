import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct GitHubTrafficSheet: View {
  let store: StoreOf<GitHubConnector>
  let traffic: Components.Schemas.GitHubTraffic
  @State private var rate: String

  init(store: StoreOf<GitHubConnector>, traffic: Components.Schemas.GitHubTraffic) {
    self.store = store
    self.traffic = traffic
    _rate = State(
      initialValue: String(
        traffic.requestedRequestsPerMinute
          ?? min(traffic.effectiveRequestsPerMinute, traffic.maximumConfigurableRequestsPerMinute)))
  }

  var body: some View {
    GitHubSheet(title: "GitHub Requests", message: store.feedback, busy: store.busy) {
      Form {
        Section {
          LabeledContent(
            "Effective limit", value: "\(traffic.effectiveRequestsPerMinute) requests/min"
          )
          .accessibilityElement(children: .combine)
          .accessibilityLabel(
            "Effective limit, \(traffic.effectiveRequestsPerMinute) requests per minute"
          )
          .accessibilityIdentifier("github-effective-rate")
          LabeledContent("Requests in parallel", value: traffic.concurrency.formatted())
          LabeledContent("Requested limit") {
            HStack(spacing: 6) {
              TextField("Maximum requests per minute", text: $rate)
                .labelsHidden()
                .multilineTextAlignment(.trailing)
                .frame(width: 64)
                .accessibilityIdentifier("github-request-rate")
              Text("requests/min").foregroundStyle(.secondary)
            }
          }
        } header: {
          Text("Local Rate Limit")
        } footer: {
          Text(
            "Choose 1–\(traffic.maximumConfigurableRequestsPerMinute) requests per minute for all repositories."
          )
        }
        Section("Request Activity") {
          LabeledContent("Attempts", value: traffic.attempts.formatted())
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Attempts, \(traffic.attempts.formatted())")
            .accessibilityIdentifier("github-request-attempts")
          LabeledContent("Failed", value: traffic.failures.formatted())
          LabeledContent("Queued", value: traffic.queued.formatted())
          LabeledContent("In flight", value: traffic.pending.formatted())
          LabeledContent("Uncertain", value: traffic.uncertain.formatted())
          if let until = traffic.cooldownUntilMs {
            LabeledContent("Cooldown ends") {
              Text(Date(timeIntervalSince1970: Double(until) / 1_000), style: .time)
            }
          }
        }
      }
      .formStyle(.grouped)
      .contentMargins(.bottom, 16, for: .scrollContent)
      .frame(minHeight: 290, idealHeight: 455, maxHeight: 500)
    } buttons: {
      Button("Refresh") { store.send(.reload) }.disabled(store.busy)
      Spacer()
      Button("Done", role: .cancel) { store.send(.dismissPanel) }
        .keyboardShortcut(.cancelAction)
        .disabled(store.busy)
      Button("Apply Limit") {
        if let requested = Int(rate) { store.send(.setRate(requested)) }
      }
      .disabled(store.busy || Int(rate) == nil || Int(rate) == traffic.effectiveRequestsPerMinute)
      .accessibilityIdentifier("github-apply-rate")
    }
  }
}
