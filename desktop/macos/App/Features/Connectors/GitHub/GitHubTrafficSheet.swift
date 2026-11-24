import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct GitHubTrafficSheet: View {
  let store: StoreOf<GitHubConnector>
  let traffic: Components.Schemas.GitHubTraffic
  @State private var rate: String
  @State private var limit = ""

  init(store: StoreOf<GitHubConnector>, traffic: Components.Schemas.GitHubTraffic) {
    self.store = store
    self.traffic = traffic
    _rate = State(
      initialValue: String(
        traffic.requestedRequestsPerMinute
          ?? min(traffic.effectiveRequestsPerMinute, traffic.maximumConfigurableRequestsPerMinute)))
  }

  var body: some View {
    GitHubSheet(
      title: "GitHub Requests & Storage", message: store.feedback, busy: store.busy,
      prefersTallContent: true
    ) {
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
              Button("Apply Limit") {
                if let requested = Int(rate) { store.send(.setRate(requested)) }
              }
              .disabled(
                Int(rate) == nil || Int(rate) == traffic.effectiveRequestsPerMinute
              )
              .accessibilityIdentifier("github-apply-rate")
            }
          }
        } header: {
          Text("Local Rate Limit")
        } footer: {
          Text(
            "Choose 1–\(traffic.maximumConfigurableRequestsPerMinute) requests per minute for all repositories."
          )
        }
        Section {
          if let storage = store.storage {
            LabeledContent("Retained payload") {
              Text(
                "\((Double(storage.bytes) / 1_000_000_000).formatted(.number.precision(.fractionLength(3)))) GB (\(storage.bytes.formatted()) bytes)"
              )
              .accessibilityIdentifier("github-storage-bytes")
            }
            LabeledContent("Storage limit") {
              HStack(spacing: 6) {
                TextField("Storage limit in decimal GB", text: $limit)
                  .labelsHidden().multilineTextAlignment(.trailing).frame(width: 64)
                  .accessibilityIdentifier("github-storage-limit")
                Text("GB").foregroundStyle(.secondary)
              }
            }
            Button("Apply Storage Limit") {
              if let value = Int(limit) { store.send(.setStorage(value)) }
            }
            .disabled(
              store.busy || !(1...1000).contains(Int(limit) ?? 0) || Int(limit) == storage.limitGb
            )
            .accessibilityIdentifier("github-apply-storage")
          } else {
            Text("Storage usage unavailable.").foregroundStyle(.secondary)
          }
        } header: {
          Text("GitHub Storage")
        } footer: {
          Text(
            "Choose 1–1,000 decimal GB. Logical payload excludes SQLite indexes, free pages, WAL and filesystem overhead. Lowering the limit does not delete data."
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
    } buttons: {
      Button("Refresh Storage") { store.send(.loadStorage) }.disabled(store.busy)
      Spacer()
      Button("Done", role: .cancel) { store.send(.dismissPanel) }
        .keyboardShortcut(.cancelAction)
        .disabled(store.busy)
    }
    .onChange(of: store.storage?.revision, initial: true) {
      if let storage = store.storage { limit = String(storage.limitGb) }
    }
  }
}
