import Foundation
import LocalAPI
import SwiftUI

struct ModelDownloadRow: View {
  let model: Components.Schemas.ModelDownload
  let requesting: Bool
  let requestPending: Bool
  let install: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 7) {
      HStack(spacing: 12) {
        VStack(alignment: .leading, spacing: 3) {
          Text(model.label ?? model.profile).font(.body.weight(.medium))
          Text(status)
            .font(.callout.monospacedDigit())
            .foregroundStyle(.secondary)
            .accessibilityIdentifier("model-status-\(model.profile)")
        }
        Spacer()
        rowAction
      }
      if model.state == .installing {
        ProgressView(value: Double(model.downloadedBytes), total: Double(model.totalBytes))
          .controlSize(.small)
          .accessibilityLabel("Download progress")
      } else if model.state == .failed {
        Label(errorMessage, systemImage: "exclamationmark.circle")
          .font(.callout)
          .foregroundStyle(.secondary)
      }
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 10)
  }

  @ViewBuilder private var rowAction: some View {
    if requesting {
      ProgressView().controlSize(.small).accessibilityLabel("Starting download")
    } else if model.state == .notInstalled || model.state == .failed {
      Button(
        model.state == .failed ? "Retry" : "Download",
        systemImage: model.state == .failed ? "arrow.clockwise" : "arrow.down",
        action: install
      )
      .buttonStyle(.bordered)
      .controlSize(.small)
      .disabled(requestPending)
      .accessibilityIdentifier("download-model-\(model.profile)")
    } else if model.state == .installed {
      Image(systemName: "checkmark.circle.fill")
        .foregroundStyle(.green)
        .accessibilityHidden(true)
    }
  }

  private var status: String {
    let total = bytes(model.totalBytes)
    let downloaded = bytes(model.downloadedBytes)
    switch model.state {
    case .notInstalled:
      return model.downloadedBytes > 0
        ? "Not installed · \(downloaded) of \(total)" : "Not installed · \(total)"
    case .installing:
      return "Downloading · \(downloaded) of \(total)"
    case .installed:
      return "Installed · \(total)"
    case .failed:
      return "Download failed · \(downloaded) of \(total)"
    }
  }

  private var errorMessage: LocalizedStringKey {
    switch model.error {
    case "model_integrity_failed": "The download did not pass verification."
    case "model_installation_unavailable": "Model installation is currently unavailable."
    case "model_transfer_failed": "Retry to resume the download."
    default: "Try downloading the model again."
    }
  }

  private func bytes(_ count: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: count, countStyle: .file)
  }
}
