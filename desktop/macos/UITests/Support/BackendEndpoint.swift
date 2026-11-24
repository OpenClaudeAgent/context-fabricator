import Foundation

struct BackendEndpoint: Decodable {
  let url: URL
  let processId: Int32
  let token: String

  static func read(in directory: URL) throws -> Self {
    try JSONDecoder().decode(
      Self.self, from: Data(contentsOf: directory.appending(path: "endpoint.json"))
    )
  }
}
