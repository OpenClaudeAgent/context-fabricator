import Foundation
import LocalAPI
import SwiftUI

struct ModelDownloadRow: View {
  @Environment(\.locale) private var locale
  let model: Components.Schemas.ModelDownload
  let storage: Components.Schemas.ModelStoragePlan?
  let requesting: Bool
  let requestPending: Bool
  let requestFeedback: String?
  let install: () -> Void
  let pause: () -> Void
  let resume: () -> Void
  let discard: () -> Void
  let delete: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 5) {
      HStack(spacing: 12) {
        VStack(alignment: .leading, spacing: 3) {
          Text(model.label ?? model.profile)
            .font(.body.weight(.medium))
            .lineLimit(1)
            .truncationMode(.middle)
          HStack(spacing: 8) {
            Text(status)
              .font(.callout.monospacedDigit())
              .foregroundStyle(.secondary)
              .accessibilityIdentifier("model-status-\(model.profile)")
            if let source = model.source, let url = URL(string: source.url) {
              Link(source.label, destination: url)
                .font(.caption)
                .lineLimit(1)
                .accessibilityIdentifier("model-source-\(model.profile)")
            }
          }
        }
        Spacer(minLength: 8)
        rowAction.fixedSize(horizontal: true, vertical: false)
      }
      if model.state == .installing || model.state == .pausing {
        ModelTransferProgress(model: model)
          .accessibilityIdentifier("model-download-progress-\(model.profile)")
      } else if model.state == .failed {
        Label(errorMessage, systemImage: "exclamationmark.circle")
          .font(.callout)
          .foregroundStyle(.secondary)
      } else if model.state == .installed, let block = model.deletionBlock {
        Text(block).font(.caption).foregroundStyle(.secondary)
          .lineLimit(1).help(block)
      }
      if let requestFeedback {
        Label(locale.localizedKey(requestFeedback), systemImage: "exclamationmark.circle")
          .font(.callout)
          .foregroundStyle(.secondary)
          .accessibilityIdentifier("model-request-error-\(model.profile)")
      }
      if let storage,
        model.state == .notInstalled || model.state == .failed
          || model.state == .paused || model.state == .interrupted
      {
        Text(
          "Peak space: \(bytes(storage.peakBytes))"
            + (storage.fits ? "" : " · Insufficient available space")
        )
        .font(.caption).foregroundStyle(.secondary)
        .help(
          storage.fits ? "" : String(localized: "A retained file may be reused after verification.")
        )
      }
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 8)
  }

  @ViewBuilder private var rowAction: some View {
    if requesting {
      ProgressView().controlSize(.small).accessibilityLabel("Updating model")
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
    } else if model.state == .installing {
      Button("Pause", systemImage: "pause", action: pause)
        .buttonStyle(.bordered).controlSize(.small).disabled(requestPending)
        .accessibilityIdentifier("pause-model-\(model.profile)")
    } else if model.state == .paused || model.state == .interrupted {
      HStack(spacing: 6) {
        Button("Resume", systemImage: "play", action: resume)
          .accessibilityIdentifier("resume-model-\(model.profile)")
        Button("Discard…", systemImage: "trash", role: .destructive, action: discard)
          .buttonStyle(.borderless)
          .accessibilityIdentifier("discard-model-partial-\(model.profile)")
      }
      .buttonStyle(.bordered).controlSize(.small).disabled(requestPending)
    } else if model.state == .pausing {
      ProgressView().controlSize(.small).accessibilityLabel("Pausing download")
    } else if model.state == .installed {
      Button("Delete Model…", systemImage: "trash", role: .destructive, action: delete)
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(requestPending || model.deletionBlock != nil)
        .accessibilityIdentifier("delete-model-\(model.profile)")
    }
  }

  private var status: String {
    let total = bytes(model.totalBytes)
    let downloaded = bytes(model.downloadedBytes)
    switch model.state {
    case .notInstalled:
      return model.downloadedBytes > 0
        ? "Not installed · \(downloaded) of \(total)"
        : locale.localized("Not installed · \(total)")
    case .installing:
      switch model.installationStage {
      case .verifying: return locale.localized("Verifying model file…")
      case .preparing: return locale.localized("Preparing download…")
      case .transferring, .none:
        return locale.localized("Downloading · \(downloaded) of \(total)")
      }
    case .pausing:
      return model.showsTransferBytes
        ? locale.localized("Pausing · \(downloaded) of \(total)")
        : locale.localized("Pausing download…")
    case .paused:
      return locale.localized("Paused · \(downloaded) of \(total)")
    case .interrupted:
      return locale.localized("Interrupted · \(downloaded) of \(total)")
    case .installed:
      return locale.localized("Installed · \(total)")
    case .failed:
      return locale.localized("Download failed · \(downloaded) of \(total)")
    }
  }

  private var errorMessage: LocalizedStringKey {
    switch model.error {
    case .model_integrity_failed: "The download did not pass verification."
    case .model_storage_limit: "Free disk space or raise the inference artifact limit in Settings."
    case .model_installation_unavailable: "Model installation is currently unavailable."
    case .model_transfer_failed: "Retry to resume the download."
    default: "Try downloading the model again."
    }
  }

  private func bytes(_ count: Int64) -> String {
    count.formatted(.byteCount(style: .file).locale(locale))
  }
}

struct ModelTransferProgress: View {
  let model: Components.Schemas.ModelDownload

  var body: some View {
    Group {
      if model.showsTransferBytes && model.downloadedBytes > 0 {
        ProgressView(
          value: Double(min(model.downloadedBytes, model.totalBytes)),
          total: Double(model.totalBytes)
        )
        .accessibilityLabel("Download progress")
      } else {
        ProgressView()
          .accessibilityLabel("Model installation progress")
      }
    }
    .progressViewStyle(.linear)
    .controlSize(.small)
  }
}

extension Components.Schemas.ModelDownload {
  var showsTransferBytes: Bool {
    switch installationStage {
    case .preparing, .verifying: false
    case .transferring: true
    case .none: state != .notInstalled
    }
  }
}
