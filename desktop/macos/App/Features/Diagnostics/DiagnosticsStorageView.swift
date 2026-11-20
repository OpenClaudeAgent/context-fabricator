import Foundation
import LocalAPI
import SwiftUI

struct DiagnosticsStorageView: View {
  let health: Components.Schemas.TelemetryHealth
  let commandsEnabled: Bool
  let purge: () -> Void

  var body: some View {
    Form {
      Section("Retention & Storage") {
        LabeledContent(
          "Stored on disk", value: health.fileBytes.formatted(.byteCount(style: .file)))
        LabeledContent("Maximum disk space") {
          Text(health.budgetBytes.formatted(.byteCount(style: .file)))
            .accessibilityIdentifier("diagnostics-budget")
        }
        if health.budgetBytes > 0 {
          ProgressView(
            value: Double(min(health.fileBytes, health.budgetBytes)),
            total: Double(health.budgetBytes)
          ) {
            Text("Disk budget used")
          }
        }
        LabeledContent("Retention") {
          Text("\(health.retentionHours.formatted()) h")
            .accessibilityIdentifier("diagnostics-retention")
        }
        if let oldest = health.oldestAt {
          LabeledContent("Oldest retained record") {
            Text(Date(timeIntervalSince1970: TimeInterval(oldest)), style: .relative)
          }
        }
        Button("Purge expired", systemImage: "clock.arrow.circlepath", action: purge)
          .disabled(!commandsEnabled)
      }
      Section("Record Handling") {
        LabeledContent("Queued records", value: health.queued.formatted())
        LabeledContent("Dropped records", value: health.dropped.formatted())
        LabeledContent("Purged records", value: health.pruned.formatted())
        if health.debugUntil > 0 {
          LabeledContent("Debug expires") {
            Text(Date(timeIntervalSince1970: TimeInterval(health.debugUntil)), style: .time)
          }
        }
      }
    }
    .formStyle(.grouped)
    .contentMargins(.bottom, 16, for: .scrollContent)
  }
}
