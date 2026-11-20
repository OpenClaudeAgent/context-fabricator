import Foundation
import LocalAPI

struct TraceStep: Identifiable {
  enum Outcome {
    case succeeded
    case failed
    case unknown

    var title: String {
      switch self {
      case .succeeded: "Succeeded"
      case .failed: "Failed"
      case .unknown: "Unknown"
      }
    }

    var symbol: String {
      switch self {
      case .succeeded: "checkmark.circle"
      case .failed: "exclamationmark.circle"
      case .unknown: "questionmark.circle"
      }
    }
  }

  let sample: Components.Schemas.SignalSample
  let depth: Int
  let parentMissing: Bool

  var id: Int64 { sample.id }
  var title: String { sample.displayName ?? Self.stageTitle(sample.name) }
  var outcome: Outcome {
    switch sample.level {
    case .ok: .succeeded
    case .error: .failed
    default: .unknown
    }
  }

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

  static func stageTitle(_ name: String) -> String {
    switch name {
    case "api_answer": "Answer request"
    case "api_query": "Search request"
    case "answer_retrieval": "Retrieve evidence"
    case "answer_generation": "Generate answer"
    case "lexical_lookup": "Lexical lookup"
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
