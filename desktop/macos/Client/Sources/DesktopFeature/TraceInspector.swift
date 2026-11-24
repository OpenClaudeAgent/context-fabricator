import ComposableArchitecture
import Foundation
import LocalAPI

@Reducer
public struct TraceInspector {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var spans: [Components.Schemas.SignalSample] = []
    public internal(set) var logs: [Components.Schemas.SignalSample] = []
    public internal(set) var nextBeforeId: Int64?
    public internal(set) var isLoading = false
    public internal(set) var feedback: String?
    public internal(set) var savedAnswer: Components.Schemas.AnswerHistoryEntry?
    public internal(set) var loadingSavedAnswer = false
    public internal(set) var savedAnswerFeedback: String?
    var savedAnswerRequest: UUID?
    var expiryToken: UUID?
    var traceID: String?

    public var hasRecords: Bool { !spans.isEmpty || !logs.isEmpty }

    public func isAnswerTrace(_ id: String) -> Bool {
      traceID == id && spans.contains { $0.name == "api_answer" }
    }
  }

  public enum Action {
    case load(String)
    case loadMore
    case loadSavedAnswer
    case savedAnswerLoaded(UUID, Components.Schemas.AnswerHistoryEntry?)
    case savedAnswerFailed(UUID)
    case savedAnswerExpired(UUID)
    case discardSavedAnswer
    case close
    case loaded(String, Components.Schemas.DiagnosticTracePage)
    case failed(String, String)
  }

  public typealias Read =
    @Sendable (String, Int64?) async throws -> Components.Schemas.DiagnosticTracePage

  public typealias ReadSavedAnswer =
    @Sendable (String) async throws -> Components.Schemas.AnswerHistoryEntry?

  private enum Cancellation: Hashable { case page, savedAnswer, expiry }
  @Dependency(\.uuid) private var uuid
  @Dependency(\.date.now) private var now
  @Dependency(\.continuousClock) private var clock
  private let read: Read
  private let readSavedAnswer: ReadSavedAnswer

  public init(
    readSavedAnswer: @escaping ReadSavedAnswer = { _ in nil },
    read: @escaping Read
  ) {
    self.read = read
    self.readSavedAnswer = readSavedAnswer
  }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .load(let id):
        state = State()
        state.traceID = id
        return .merge(
          .cancel(id: Cancellation.savedAnswer), .cancel(id: Cancellation.expiry),
          load(&state, id: id))
      case .loadSavedAnswer:
        guard let traceID = state.traceID else { return .none }
        let request = uuid()
        state.savedAnswerRequest = request
        state.expiryToken = nil
        state.savedAnswer = nil
        state.savedAnswerFeedback = nil
        state.loadingSavedAnswer = true
        return .merge(
          .cancel(id: Cancellation.expiry),
          .run { [readSavedAnswer] send in
            do {
              let entry = try await readSavedAnswer(traceID)
              try Task.checkCancellation()
              await send(.savedAnswerLoaded(request, entry))
            } catch {
              if !Task.isCancelled { await send(.savedAnswerFailed(request)) }
            }
          }.cancellable(id: Cancellation.savedAnswer, cancelInFlight: true))
      case .savedAnswerLoaded(let request, let entry):
        guard state.savedAnswerRequest == request else { return .none }
        state.savedAnswerRequest = nil
        state.loadingSavedAnswer = false
        state.savedAnswer = entry
        guard let expiresAtMs = entry?.expiresAtMs else { return .none }
        let token = uuid()
        state.expiryToken = token
        let delay = max(0, Double(expiresAtMs) / 1000 - now.timeIntervalSince1970)
        let expiryClock = clock
        return .run { send in
          try await expiryClock.sleep(for: .seconds(delay))
          await send(.savedAnswerExpired(token))
        }.cancellable(id: Cancellation.expiry, cancelInFlight: true)
      case .savedAnswerFailed(let request):
        guard state.savedAnswerRequest == request else { return .none }
        state.savedAnswerRequest = nil
        state.loadingSavedAnswer = false
        state.savedAnswerFeedback = "Could not load the saved answer."
        return .none
      case .savedAnswerExpired(let token):
        guard state.expiryToken == token else { return .none }
        state.savedAnswer = nil
        state.expiryToken = nil
        return .none
      case .discardSavedAnswer:
        state.savedAnswer = nil
        state.savedAnswerRequest = nil
        state.expiryToken = nil
        state.loadingSavedAnswer = false
        state.savedAnswerFeedback = nil
        return .merge(.cancel(id: Cancellation.savedAnswer), .cancel(id: Cancellation.expiry))
      case .loadMore:
        guard let id = state.traceID, let before = state.nextBeforeId, !state.isLoading
        else { return .none }
        return load(&state, id: id, before: before)
      case .close:
        state = State()
        return .merge(
          .cancel(id: Cancellation.page), .cancel(id: Cancellation.savedAnswer),
          .cancel(id: Cancellation.expiry))
      case .loaded(let id, let page):
        guard id == state.traceID else { return .none }
        state.spans.append(contentsOf: page.spans)
        state.logs.append(contentsOf: page.logs)
        state.nextBeforeId = page.nextBeforeId
        state.isLoading = false
        return .none
      case .failed(let id, let message):
        guard id == state.traceID else { return .none }
        state.isLoading = false
        state.feedback = message
        return .none
      }
    }
  }

  private func load(_ state: inout State, id: String, before: Int64? = nil) -> EffectOf<Self> {
    state.isLoading = true
    state.feedback = nil
    let retryClock = clock
    return .run { [read] send in
      do {
        var page = try await read(id, before)
        if before == nil && !page.spans.contains(where: { $0.parentSpanId == nil }) {
          // Telemetry writes run on the local sink's background flush interval.
          try await retryClock.sleep(for: .milliseconds(350))
          page = try await read(id, nil)
        }
        try Task.checkCancellation()
        await send(.loaded(id, page))
      } catch {
        if !Task.isCancelled { await send(.failed(id, error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.page, cancelInFlight: true)
  }
}
