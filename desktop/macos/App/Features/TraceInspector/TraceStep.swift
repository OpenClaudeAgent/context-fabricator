import Foundation
import LocalAPI
import SwiftUI

struct TraceStep: Identifiable {
  typealias Outcome = Components.Schemas.DiagnosticSpanOutcome

  let sample: Components.Schemas.SignalSample
  let depth: Int
  let parentMissing: Bool

  var id: Int64 { sample.id }
  func title(in locale: Locale) -> String { Self.title(for: sample, in: locale) }

  static func title(for sample: Components.Schemas.SignalSample, in locale: Locale) -> String {
    if sample.name == "api_request", let method = sample.httpMethod, let route = sample.httpRoute {
      return "\(method) \(route)"
    }
    return sample.displayName.map { locale.localizedKey($0) }
      ?? stageTitle(sample.name, in: locale)
  }
  var outcome: Outcome { sample.spanOutcome ?? .unknown }

  static func outline(_ spans: [Components.Schemas.SignalSample]) -> [Self] {
    let ordered = spans.sorted { startOrder($0) < startOrder($1) }
    let knownIDs = Set(ordered.compactMap(\.spanId))
    let byParent = Dictionary(grouping: ordered, by: \.parentSpanId)
    var visited = Set<Int64>()
    var result: [Self] = []

    func append(_ span: Components.Schemas.SignalSample, depth: Int, parentMissing: Bool = false) {
      guard visited.insert(span.id).inserted else { return }
      result.append(.init(sample: span, depth: depth, parentMissing: parentMissing))
      if let id = span.spanId {
        for child in byParent[id] ?? [] { append(child, depth: depth + 1) }
      }
    }

    for span in ordered {
      if let parent = span.parentSpanId {
        if !knownIDs.contains(parent) { append(span, depth: 0, parentMissing: true) }
      } else {
        append(span, depth: 0)
      }
    }
    for span in ordered { append(span, depth: 0, parentMissing: true) }
    return result
  }

  private static func startOrder(_ span: Components.Schemas.SignalSample) -> (Double, Int64) {
    (span.startedAtUnixMicros.map(Double.init) ?? Double(span.timestamp) * 1_000_000, span.id)
  }

  private static func stageTitle(_ name: String, in locale: Locale) -> String {
    switch name {
    case "api_answer": locale.localized("Answer request")
    case "api_query": locale.localized("Search request")
    case "api_request": locale.localized("Local API request")
    case "answer_retrieval": locale.localized("Retrieve evidence")
    case "answer_generation": locale.localized("Generate answer")
    case "lexical_lookup": locale.localized("Lexical lookup")
    default: name.replacingOccurrences(of: "_", with: " ").capitalized
    }
  }

  static func elapsed(_ micros: Int64) -> String {
    switch micros {
    case ..<1_000:
      "\(micros.formatted()) µs"
    case ..<1_000_000:
      "\((Double(micros) / 1_000).formatted(.number.precision(.fractionLength(0...3)))) ms"
    default:
      "\((Double(micros) / 1_000_000).formatted(.number.precision(.fractionLength(0...6)))) s"
    }
  }
}

extension Components.Schemas.DiagnosticSpanOutcome {
  func title(in locale: Locale) -> String {
    switch self {
    case .succeeded: locale.localized("Succeeded")
    case .failed: locale.localized("Failed")
    case .insufficientEvidence: locale.localized("Insufficient evidence")
    case .cancelled: locale.localized("Cancelled")
    case .unknown: locale.localized("Unknown")
    }
  }

  var symbol: String {
    switch self {
    case .succeeded: "checkmark.circle"
    case .failed, .insufficientEvidence: "exclamationmark.circle"
    case .cancelled: "xmark.circle"
    case .unknown: "questionmark.circle"
    }
  }

  var tint: Color {
    switch self {
    case .succeeded: .green
    case .failed: .red
    case .insufficientEvidence: .orange
    case .cancelled, .unknown: .secondary
    }
  }
}
