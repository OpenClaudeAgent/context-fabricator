import Foundation

public enum BackendConnectionError: LocalizedError, Equatable, Sendable {
  case helperMissing
  case helperExited
  case launchFailed
  case startTimedOut
  case stopTimedOut
  case incompatibleAPI

  public var errorDescription: String? {
    switch self {
    case .helperMissing: "The bundled backend is missing."
    case .helperExited: "The local backend exited. Check that its data directory is available."
    case .launchFailed: "The bundled backend could not be launched."
    case .startTimedOut: "The local backend did not become ready."
    case .stopTimedOut: "The local backend took too long to stop. Try restarting it again."
    case .incompatibleAPI: "The local backend uses an incompatible API version."
    }
  }
}
