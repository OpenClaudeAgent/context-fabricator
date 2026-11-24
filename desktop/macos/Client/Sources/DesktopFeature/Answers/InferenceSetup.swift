import ComposableArchitecture
import Foundation
import LocalAPI

public struct InferenceSetupClient: Sendable {
  let plan:
    @Sendable (Components.Schemas.InferenceSetupSelection) async throws ->
      Components.Schemas.InferenceSetupPlan
  let start:
    @Sendable (Components.Schemas.InferenceSetupSelection, String, UUID, String?) async throws ->
      Components.Schemas.InferenceSetupStatus
  let status: @Sendable () async throws -> Components.Schemas.InferenceSetupStatus?
  let provideArchive:
    @Sendable (String, String) async throws -> Components.Schemas.InferenceSetupStatus
  let cancelArchive: @Sendable (String) async throws -> Components.Schemas.InferenceSetupStatus

  static let unavailable = InferenceSetupClient(
    plan: { _ in throw BackendConnectionError.helperExited },
    start: { _, _, _, _ in throw BackendConnectionError.helperExited },
    status: { nil }
  )

  @preconcurrency public init(
    plan:
      @escaping @Sendable (Components.Schemas.InferenceSetupSelection) async throws ->
      Components.Schemas.InferenceSetupPlan,
    start:
      @escaping @Sendable (Components.Schemas.InferenceSetupSelection, String, UUID, String?)
      async throws -> Components.Schemas.InferenceSetupStatus,
    status: @escaping @Sendable () async throws -> Components.Schemas.InferenceSetupStatus?,
    provideArchive:
      @escaping @Sendable (String, String) async throws -> Components.Schemas.InferenceSetupStatus =
      { _, _ in throw BackendConnectionError.helperExited },
    cancelArchive:
      @escaping @Sendable (String) async throws -> Components.Schemas.InferenceSetupStatus = { _ in
        throw BackendConnectionError.helperExited
      }
  ) {
    self.plan = plan
    self.start = start
    self.status = status
    self.provideArchive = provideArchive
    self.cancelArchive = cancelArchive
  }
}

@Reducer
public struct InferenceSetup {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var plan: Components.Schemas.InferenceSetupPlan?
    public internal(set) var status: Components.Schemas.InferenceSetupStatus?
    public internal(set) var requesting = false
    public internal(set) var starting = false
    public var inProgress: Bool {
      guard let status else { return false }
      switch status.phase {
      case .runtime, .answerWeights, .activatingAnswer, .embeddingWeights, .waitingForArchive:
        return true
      case .completed, .failed, .interrupted: return false
      }
    }
    public internal(set) var feedback: String?
    var archivePath: String?
    var observing = false
  }
  public enum Action {
    case observe, stop, cancel, confirm
    case provideArchive(String, String)
    case cancelArchive(String)
    case review(Components.Schemas.InferenceSetupSelection, String?)
    case planLoaded(Components.Schemas.InferenceSetupPlan)
    case statusLoaded(Components.Schemas.InferenceSetupStatus?)
    case statusPolled(Components.Schemas.InferenceSetupStatus?)
    case pollingFailed(String)
    case failed(String)
  }
  private enum CancelID { case plan, observation }
  private let client: InferenceSetupClient
  @Dependency(\.continuousClock) private var clock
  @Dependency(\.uuid) private var uuid
  public init(client: InferenceSetupClient) { self.client = client }
  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .observe:
        state.observing = true
        return observe()
      case .stop:
        state.observing = false
        state.plan = nil
        state.archivePath = nil
        if !state.starting { state.requesting = false }
        return .merge(.cancel(id: CancelID.plan), .cancel(id: CancelID.observation))
      case .review(let selection, let path):
        guard !state.requesting, !state.inProgress else { return .none }
        state.plan = nil
        state.archivePath = path
        state.feedback = nil
        state.requesting = true
        return .run { [client] send in
          do {
            let plan = try await client.plan(selection)
            try Task.checkCancellation()
            await send(.planLoaded(plan))
          } catch is CancellationError {} catch {
            if !Task.isCancelled { await send(.failed(error.localizedDescription)) }
          }
        }.cancellable(id: CancelID.plan, cancelInFlight: true)
      case .planLoaded(let plan):
        guard state.requesting else { return .none }
        state.requesting = false
        state.plan = plan
        return .none
      case .cancel:
        guard !state.starting else { return .none }
        state.plan = nil
        state.archivePath = nil
        state.requesting = false
        return .cancel(id: CancelID.plan)
      case .confirm:
        guard !state.requesting, !state.inProgress, let plan = state.plan, plan.canStart,
          !(plan.runtimeRequired && plan.selection.runtimeSource == .localArchive
            && state.archivePath == nil)
        else { return .none }
        let path = state.archivePath
        let operationID = uuid()
        state.plan = nil
        state.archivePath = nil
        state.requesting = true
        state.starting = true
        state.feedback = nil
        let request: EffectOf<Self> = .run { [client] send in
          do {
            await send(
              .statusLoaded(
                try await client.start(plan.selection, plan.fingerprint, operationID, path)))
          } catch { await send(.failed(error.localizedDescription)) }
        }
        return .merge(.cancel(id: CancelID.observation), request)
      case .provideArchive(let operationID, let path):
        guard !state.requesting, let status = state.status,
          status.phase == .waitingForArchive, status.operationId == operationID, !path.isEmpty
        else { return .none }
        state.requesting = true
        state.starting = true
        state.feedback = nil
        let request: EffectOf<Self> = .run { [client] send in
          do { await send(.statusLoaded(try await client.provideArchive(operationID, path))) } catch
          { await send(.failed(error.localizedDescription)) }
        }
        return .merge(.cancel(id: CancelID.observation), request)
      case .cancelArchive(let operationID):
        guard !state.requesting, let status = state.status,
          status.phase == .waitingForArchive, status.operationId == operationID
        else { return .none }
        state.requesting = true
        state.starting = true
        state.feedback = nil
        let request: EffectOf<Self> = .run { [client] send in
          do { await send(.statusLoaded(try await client.cancelArchive(operationID))) } catch {
            await send(.failed(error.localizedDescription))
          }
        }
        return .merge(.cancel(id: CancelID.observation), request)
      case .statusLoaded(let status):
        state.requesting = false
        state.starting = false
        state.status = status
        return state.observing ? observe() : .none
      case .statusPolled(let status):
        if !state.requesting { state.status = status }
        return .none
      case .pollingFailed(let message):
        if !state.requesting { state.feedback = message }
        return .none
      case .failed(let message):
        state.requesting = false
        state.starting = false
        state.feedback = message
        return state.observing ? observe() : .none
      }
    }
  }
  private func observe() -> EffectOf<Self> {
    let clock = clock
    return .run { [client] send in
      while !Task.isCancelled {
        do {
          let status = try await client.status()
          try Task.checkCancellation()
          await send(.statusPolled(status))
        } catch is CancellationError { return } catch {
          await send(.pollingFailed(error.localizedDescription))
        }
        try await clock.sleep(for: .seconds(1))
      }
    }.cancellable(id: CancelID.observation, cancelInFlight: true)
  }
}
