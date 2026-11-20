import ComposableArchitecture
import Foundation
import LocalAPI

public struct ModelLibraryClient: Sendable {
  public typealias Catalog = @Sendable () async throws -> [Components.Schemas.ModelDownload]
  public typealias Install = @Sendable (String) async throws -> Void

  let catalog: Catalog
  let install: Install

  public init(catalog: @escaping Catalog, install: @escaping Install) {
    self.catalog = catalog
    self.install = install
  }
}

@Reducer
public struct ModelLibrary {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var profiles: [Components.Schemas.ModelDownload]?
    public internal(set) var pendingProfile: String?
    public internal(set) var feedback: String?
    public internal(set) var catalogFeedback: String?
    var isObserving = false
    var acceptedProfile: String?
  }

  public enum Action {
    case observe
    case stopObserving
    case install(String)
    case catalogLoaded([Components.Schemas.ModelDownload])
    case catalogFailed(String)
    case installationAccepted(String)
    case installationRejected(String, String)
  }

  private enum Cancellation: Hashable { case observation }

  private let client: ModelLibraryClient
  @Dependency(\.continuousClock) private var clock

  public init(client: ModelLibraryClient) { self.client = client }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .observe:
        state.isObserving = true
        state.catalogFeedback = nil
        return observe()
      case .stopObserving:
        state.isObserving = false
        state.profiles = nil
        if state.acceptedProfile != nil {
          state.pendingProfile = nil
          state.acceptedProfile = nil
        }
        return .cancel(id: Cancellation.observation)
      case .install(let profile):
        guard state.pendingProfile == nil,
          let model = state.profiles?.first(where: {
            $0.profile == profile
          }), model.state == .notInstalled || model.state == .failed
        else { return .none }
        state.pendingProfile = profile
        state.feedback = nil
        return install(profile)
      case .catalogLoaded(let profiles):
        state.catalogFeedback = nil
        state.profiles = profiles
        if state.acceptedProfile != nil {
          state.pendingProfile = nil
          state.acceptedProfile = nil
        }
        return .none
      case .catalogFailed(let message):
        state.catalogFeedback = message
        return .none
      case .installationAccepted(let profile):
        guard state.pendingProfile == profile else { return .none }
        guard state.isObserving else {
          state.pendingProfile = nil
          return .none
        }
        state.acceptedProfile = profile
        return observe()
      case .installationRejected(let profile, let message):
        if state.pendingProfile == profile { state.pendingProfile = nil }
        state.feedback = message
        return .none
      }
    }
  }

  private func observe() -> EffectOf<Self> {
    let pollingClock = clock
    return .run { [client] send in
      while !Task.isCancelled {
        do {
          let profiles = try await client.catalog()
          try Task.checkCancellation()
          await send(.catalogLoaded(profiles))
        } catch is CancellationError {
          return
        } catch {
          await send(.catalogFailed(error.localizedDescription))
        }
        try await pollingClock.sleep(for: .seconds(1))
      }
    }
    .cancellable(id: Cancellation.observation, cancelInFlight: true)
  }

  private func install(_ profile: String) -> EffectOf<Self> {
    .run { [client] send in
      do {
        try await client.install(profile)
        await send(.installationAccepted(profile))
      } catch {
        await send(.installationRejected(profile, error.localizedDescription))
      }
    }
  }
}
