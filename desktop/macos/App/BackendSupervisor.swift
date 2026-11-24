import DesktopFeature
import Foundation
import LocalAPI

@MainActor
final class BackendSupervisor {
  private struct Endpoint: Decodable {
    let apiVersion: Int
    let processId: Int32
    let url: URL
    let token: String
  }

  private let directory: URL
  private let helper: URL?
  private let fixtureArguments: [String]
  private var process: Process?
  private var parentPipe: Pipe?
  private var observers: [UUID: AsyncStream<BackendConnectionError>.Continuation] = [:]

  var pipelinePreferencesKey: String {
    "inspectionSources.\(directory.standardizedFileURL.path)"
  }

  var inferenceDirectory: URL {
    ProcessInfo.processInfo.environment["CTXFAB_INFERENCE_DIRECTORY"].map {
      URL(filePath: $0, directoryHint: .isDirectory)
    } ?? directory.appending(path: "inference", directoryHint: .isDirectory)
  }

  init(directory: URL, helper: URL?, fixtureArguments: [String]) {
    self.directory = directory
    self.helper = helper
    self.fixtureArguments = fixtureArguments
  }

  func exits() -> AsyncStream<BackendConnectionError> {
    let id = UUID()
    let (stream, continuation) = AsyncStream.makeStream(
      of: BackendConnectionError.self, bufferingPolicy: .bufferingNewest(1))
    observers[id] = continuation
    continuation.onTermination = { [weak self] _ in
      Task { @MainActor in self?.observers.removeValue(forKey: id) }
    }
    return stream
  }

  func restart() async throws -> SessionClient {
    stop()
    let deadline = ContinuousClock.now + .seconds(4)
    while process?.isRunning == true {
      guard ContinuousClock.now < deadline else { throw BackendConnectionError.stopTimedOut }
      try await Task.sleep(for: .milliseconds(50))
    }
    try Task.checkCancellation()
    return try await connect()
  }

  func connect(startIfNeeded: Bool = true) async throws -> SessionClient {
    if startIfNeeded, process?.isRunning != true {
      try launch()
    }
    guard let current = process, parentPipe != nil else {
      throw BackendConnectionError.helperExited
    }
    let deadline = ContinuousClock.now + .seconds(5)
    while ContinuousClock.now < deadline {
      try Task.checkCancellation()
      guard process === current, current.isRunning, parentPipe != nil else {
        throw exitError(current)
      }
      if let endpoint = try? readEndpoint(), endpoint.processId == current.processIdentifier {
        guard endpoint.apiVersion == 1 else { throw BackendConnectionError.incompatibleAPI }
        let client = SessionClient(url: endpoint.url, token: endpoint.token)
        if (try? await client.status()) != nil, current.isRunning, parentPipe != nil,
          process === current
        {
          return client
        }
      }
      try await Task.sleep(for: .milliseconds(100))
    }
    throw BackendConnectionError.startTimedOut
  }

  func stop() {
    try? parentPipe?.fileHandleForWriting.close()
    parentPipe = nil
  }

  private func launch() throws {
    guard let helper else { throw BackendConnectionError.helperMissing }
    let task = Process()
    let pipe = Pipe()
    task.executableURL = helper
    task.arguments = [directory.path, "--app-session"] + fixtureArguments
    task.standardInput = pipe
    task.standardOutput = FileHandle.nullDevice
    task.standardError = FileHandle.nullDevice
    task.terminationHandler = { [weak self] task in
      Task { @MainActor in self?.didExit(task) }
    }
    do {
      try task.run()
    } catch {
      throw BackendConnectionError.launchFailed
    }
    try? pipe.fileHandleForReading.close()
    process = task
    parentPipe = pipe
  }

  private func didExit(_ task: Process) {
    guard process === task else { return }
    process = nil
    let unexpected = parentPipe != nil
    stop()
    if unexpected {
      for observer in observers.values {
        observer.yield(exitError(task))
      }
    }
  }

  private func exitError(_ task: Process) -> BackendConnectionError {
    guard !task.isRunning, task.terminationReason == .exit, task.terminationStatus == 75 else {
      return .helperExited
    }
    return .dataDirectoryInUse
  }

  private func readEndpoint() throws -> Endpoint {
    let endpoint = try JSONDecoder().decode(
      Endpoint.self, from: Data(contentsOf: directory.appending(path: "endpoint.json"))
    )
    guard endpoint.url.scheme == "http", endpoint.url.host == "127.0.0.1",
      endpoint.url.port != nil, endpoint.url.user == nil, endpoint.url.password == nil,
      endpoint.url.path.isEmpty, endpoint.url.query == nil, endpoint.url.fragment == nil
    else { throw URLError(.badURL) }
    return endpoint
  }
}
