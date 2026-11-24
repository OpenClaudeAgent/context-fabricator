import DesktopFeature
import LocalAPI
import SwiftUI

extension Dashboard.Destination {
  var localizedTitle: LocalizedStringKey {
    switch self {
    case .overview: "Overview"
    case .answers: "Answers"
    case .connectors: "Connectors"
    case .diagnostics: "Diagnostics"
    case .settings: "Settings"
    }
  }
}

extension Dashboard.DiagnosticTab {
  var localizedTitle: LocalizedStringKey {
    switch self {
    case .requests: "Requests"
    case .logs: "Logs"
    case .metrics: "Metrics"
    case .traces: "Traces"
    case .storage: "Storage"
    case .answerUsage: "Answer Usage"
    }
  }
}

extension Components.Schemas.SignalSample.levelPayload {
  var localizedTitle: LocalizedStringKey {
    switch self {
    case .ok: "Ok"
    case .info: "Info"
    case .error: "Error"
    case .debug: "Debug"
    }
  }
}
