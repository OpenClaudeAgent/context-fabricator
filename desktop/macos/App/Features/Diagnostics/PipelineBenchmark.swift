import Foundation
import LocalAPI

struct PipelineBenchmark: Decodable, Sendable {
  struct Question: Codable, Sendable {
    let title: String
    let query: String
    let expected: String?
    let source: URL?
  }

  struct Reference: Codable, Sendable {
    let title: String
    let repository: String
    let revision: String
    let suite: String?
    let questions: [Question]

    var source: Components.Schemas.BenchmarkReference {
      .init(repository: repository, revision: revision)
    }

    static let bundled: [Self] = {
      guard let url = Bundle.main.url(forResource: "PipelineBenchmarks", withExtension: "json"),
        let data = try? Data(contentsOf: url)
      else { return [] }
      return (try? JSONDecoder().decode([Self].self, from: data)) ?? []
    }()
  }

  typealias Document = Components.Schemas.BenchmarkDocument

  let documents: [Document]?
  let questions: [Question]
  let reference: Components.Schemas.BenchmarkReference?

  static let maxFiles = 64
  static let maxFileBytes = 256 * 1024
  static let maxCorpusBytes = 512 * 1024

  static func embeddingUnavailable(_ error: any Error) -> Bool {
    switch (error as? AnswerRejection)?.problemCode {
    case "embedding_not_configured", "embedding_runtime_unavailable", "embedding_index_disabled":
      true
    default:
      false
    }
  }

  static func load(_ url: URL) throws -> Self {
    let suite = try JSONDecoder().decode(Self.self, from: read(url, limit: 3 * 1024 * 1024))
    if let documents = suite.documents { try validate(documents) }
    if let reference = suite.reference {
      let revision = reference.revision.utf8
      guard suite.documents == nil, reference.repository.split(separator: "/").count == 2,
        revision.count == 40,
        revision.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) })
      else { throw ImportError.reference }
    }
    guard (1...128).contains(suite.questions.count),
      suite.questions.allSatisfy({
        !$0.title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
          && $0.title.count <= 160
          && !($0.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
          && $0.query.unicodeScalars.count <= 512
      })
    else { throw ImportError.questions }
    return suite
  }

  static func files(_ urls: [URL]) throws -> [Document] {
    guard urls.count <= maxFiles else { throw ImportError.corpus }
    var documents: [Document] = []
    var remaining = maxCorpusBytes
    for url in urls {
      let data = try read(url, limit: min(maxFileBytes, remaining))
      guard let text = String(data: data, encoding: .utf8) else { throw ImportError.encoding }
      documents.append(.init(path: url.lastPathComponent, text: text))
      remaining -= data.count
    }
    try validate(documents)
    return documents
  }

  static func validate(_ documents: [Document]) throws {
    guard (1...maxFiles).contains(documents.count),
      documents.reduce(0, { $0 + $1.text.utf8.count }) <= maxCorpusBytes,
      documents.allSatisfy({ document in
        document.path.utf8.count <= 1024
          && !document.path.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
          && !document.path.split(separator: "/", omittingEmptySubsequences: false).contains {
            $0.isEmpty || $0 == "." || $0 == ".."
          }
          && !document.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
          && !document.text.contains("\0")
          && document.text.utf8.count <= maxFileBytes
      })
    else { throw ImportError.corpus }

    guard Set(documents.map(\.path)).count == documents.count else {
      throw ImportError.duplicatePath
    }
  }

  struct FixtureCase: Decodable, Identifiable, Sendable {
    struct ActionTarget: Decodable, Sendable {
      let repository: String
      let runID: Int64
      let attempt: Int
    }

    let suite: String
    let id: String
    let surface: String
    let query: String
    let expected: String?
    let oracle: String
    let action: ActionTarget?
  }

  struct Catalog: Decodable, Sendable {
    let references: [Reference]
    let fixtureCases: [FixtureCase]
    let workflowQuestionTemplates: [String: String]?

    static let bundled: Self = {
      guard
        let url = Bundle.main.url(forResource: "PipelineBenchmarkSuites", withExtension: "json"),
        let data = try? Data(contentsOf: url),
        let catalog = try? JSONDecoder().decode(Self.self, from: data)
      else { return .init(references: [], fixtureCases: [], workflowQuestionTemplates: nil) }
      return catalog
    }()
  }

  private static func read(_ url: URL, limit: Int) throws -> Data {
    let granted = url.startAccessingSecurityScopedResource()
    defer { if granted { url.stopAccessingSecurityScopedResource() } }
    guard try url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true else {
      throw ImportError.encoding
    }
    let file = try FileHandle(forReadingFrom: url)
    defer { try? file.close() }
    let data = try file.read(upToCount: limit + 1) ?? Data()
    guard data.count <= limit else { throw ImportError.corpus }
    return data
  }

  enum ImportError: LocalizedError {
    case corpus, duplicatePath, encoding, questions, reference

    var errorDescription: String? {
      switch self {
      case .corpus:
        "Use 1 to 64 text files with relative paths, up to 256 KiB per file and 512 KiB total."
      case .duplicatePath: "Each document needs a distinct path."
      case .encoding: "Select UTF-8 text files."
      case .questions: "Use 1 to 128 questions with titles and queries of at most 512 characters."
      case .reference:
        "Use a GitHub owner/repository and a full commit ID for a benchmark reference."
      }
    }
  }
}
