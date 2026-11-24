import ComposableArchitecture
import LocalAPI

public struct DashboardSnapshot: Equatable, Sendable {
  public internal(set) var status: Components.Schemas.Status
  public let settings: Components.Schemas.Settings
  public let diagnostics: [Components.Schemas.Diagnostic]
  public internal(set) var telemetry: Components.Schemas.TelemetrySnapshot?

  public init(
    status: Components.Schemas.Status, settings: Components.Schemas.Settings,
    diagnostics: [Components.Schemas.Diagnostic],
    telemetry: Components.Schemas.TelemetrySnapshot? = nil
  ) {
    self.status = status
    self.settings = settings
    self.diagnostics = diagnostics
    self.telemetry = telemetry
  }
}

public struct DashboardClient: Sendable {
  let snapshot: @Sendable () async throws -> DashboardSnapshot
  let readStatus: @Sendable () async throws -> Components.Schemas.Status
  let loadEmbeddingModels: @Sendable () async throws -> Components.Schemas.EmbeddingModelSelection
  let selectEmbeddingModel:
    @Sendable (String) async throws -> Components.Schemas.EmbeddingModelSelection
  let save: @Sendable (Components.Schemas.Settings) async throws -> Components.Schemas.Settings
  let restart: @Sendable () async throws -> DashboardSnapshot
  let exits: @Sendable () async -> AsyncStream<BackendConnectionError>
  let github: @Sendable (GitHubCommand) async throws -> GitHubResult
  let answers: AnswersClient
  let answerHistory: AnswerHistoryClient
  let modelLibrary: ModelLibraryClient
  let answerUsage: AnswerUsage.Read
  let diagnosticLogs: DiagnosticLogs.Read
  let readTrace: TraceInspector.Read
  let readTelemetry: @Sendable () async throws -> Components.Schemas.TelemetrySnapshot
  let telemetry:
    @Sendable (Components.Schemas.TelemetryControl.commandPayload) async throws ->
      Components.Schemas.TelemetrySnapshot

  // The app and this package both target Swift 6. Sendability is part of their contract.
  // swiftlint:disable:next incompatible_concurrency_annotation
  public init(
    snapshot: @escaping @Sendable () async throws -> DashboardSnapshot,
    save:
      @escaping @Sendable (Components.Schemas.Settings) async throws -> Components.Schemas.Settings,
    restart: @escaping @Sendable () async throws -> DashboardSnapshot,
    exits: @escaping @Sendable () async -> AsyncStream<BackendConnectionError>,
    loadEmbeddingModels:
      @escaping @Sendable () async throws -> Components.Schemas.EmbeddingModelSelection =
      {
        throw BackendConnectionError.helperExited
      },
    selectEmbeddingModel:
      @escaping @Sendable (String) async throws -> Components.Schemas.EmbeddingModelSelection = {
        _ in
        throw BackendConnectionError.helperExited
      },
    readStatus: (@Sendable () async throws -> Components.Schemas.Status)? = nil,
    telemetry:
      @escaping @Sendable (Components.Schemas.TelemetryControl.commandPayload) async throws ->
      Components.Schemas.TelemetrySnapshot =
      { _ in throw BackendConnectionError.helperExited },
    answers: AnswersClient = .init(),
    answerHistory: AnswerHistoryClient = .init(),
    github: @escaping @Sendable (GitHubCommand) async throws -> GitHubResult =
      { _ in throw BackendConnectionError.helperExited },
    modelLibrary: ModelLibraryClient = .init(
      catalog: { [] },
      install: { _ in
        throw BackendConnectionError.helperExited
      }),
    answerUsage: @escaping AnswerUsage.Read = { .init(groups: []) },
    diagnosticLogs: @escaping DiagnosticLogs.Read = { _, _, _ in .init(records: []) },
    readTrace: @escaping TraceInspector.Read = { _, _ in throw BackendConnectionError.helperExited
    },
    readTelemetry: @escaping @Sendable () async throws -> Components.Schemas.TelemetrySnapshot = {
      throw BackendConnectionError.helperExited
    }
  ) {
    self.snapshot = snapshot
    self.readStatus = readStatus ?? { try await snapshot().status }
    self.loadEmbeddingModels = loadEmbeddingModels
    self.selectEmbeddingModel = selectEmbeddingModel
    self.save = save
    self.restart = restart
    self.exits = exits
    self.telemetry = telemetry
    self.github = github
    self.answers = answers
    self.answerHistory = answerHistory
    self.modelLibrary = modelLibrary
    self.answerUsage = answerUsage
    self.diagnosticLogs = diagnosticLogs
    self.readTrace = readTrace
    self.readTelemetry = readTelemetry
  }
}

@Reducer
public struct Dashboard {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var snapshot: DashboardSnapshot?
    public internal(set) var github = GitHubConnector.State()
    public internal(set) var answers = Answers.State()
    public internal(set) var answerHistory = AnswerHistory.State()
    public internal(set) var modelLibrary = ModelLibrary.State()
    public internal(set) var diagnosticLogs = DiagnosticLogs.State()
    public internal(set) var answerRequests = DiagnosticLogs.State()
    public internal(set) var answerUsage = AnswerUsage.State()
    public internal(set) var traceInspector = TraceInspector.State()
    public internal(set) var settingsForm: SettingsForm.State
    public internal(set) var connection: Connection = .connecting
    public internal(set) var destination: Destination = .overview
    public internal(set) var telemetryFeedback: String?
    public internal(set) var telemetryBusy = false
    public internal(set) var diagnosticTab: DiagnosticTab = .requests
    fileprivate var showsReadyLogs: Bool {
      destination == .diagnostics && diagnosticTab == .logs && connection == .ready
    }

    fileprivate var showsReadyUsage: Bool {
      destination == .diagnostics && diagnosticTab == .answerUsage && connection == .ready
    }

    public init(settings: Components.Schemas.Settings? = nil) {
      self.settingsForm = .init(confirmed: settings)
    }
  }

  public enum Connection: Equatable {
    case connecting
    case ready
    case dataDirectoryInUse
    case unavailable(String)

    public var failureMessage: String? {
      switch self {
      case .connecting, .ready: nil
      case .dataDirectoryInUse: BackendConnectionError.dataDirectoryInUse.localizedDescription
      case .unavailable(let message): message
      }
    }
  }

  public enum Destination: String, CaseIterable, Sendable {
    case overview = "Overview"
    case answers = "Answers"
    case connectors = "Connectors"
    case diagnostics = "Diagnostics"
    case settings = "Settings"
  }

  public enum DiagnosticTab: String, CaseIterable, Sendable {
    case requests = "Requests"
    case logs = "Logs"
    case metrics = "Metrics"
    case traces = "Traces"
    case storage = "Storage"
    case answerUsage = "Answer Usage"
  }

  public enum Action {
    case github(GitHubConnector.Action)
    case answers(Answers.Action)
    case answerHistory(AnswerHistory.Action)
    case modelLibrary(ModelLibrary.Action)
    case diagnosticLogs(DiagnosticLogs.Action)
    case answerRequests(DiagnosticLogs.Action)
    case answerUsage(AnswerUsage.Action)
    case traceInspector(TraceInspector.Action)
    case destinationChanged(Destination)
    case observeBackend
    case observeOverview
    case overviewStatusLoaded(Components.Schemas.Status)
    case refresh
    case restart
    case loaded(DashboardSnapshot)
    case backendExited(BackendConnectionError)
    case loadFailed(String)
    case settingsForm(SettingsForm.Action)
    case diagnosticTabChanged(DiagnosticTab)
    case readTelemetry
    case telemetryRead(Components.Schemas.TelemetrySnapshot)
    case telemetryCommand(Components.Schemas.TelemetryControl.commandPayload)
    case telemetryUpdated(Components.Schemas.TelemetrySnapshot)
    case telemetryFailed(String)

  }

  private enum Cancellation: Hashable {
    case refresh, overview
  }

  @Dependency(\.continuousClock) private var clock
  let client: DashboardClient

  public init(client: DashboardClient) {
    self.client = client
  }

  public var body: some ReducerOf<Self> {
    Scope(state: \.github, action: \.github) { GitHubConnector(perform: client.github) }
    Scope(state: \.answers, action: \.answers) { Answers(client: client.answers) }
    Scope(state: \.answerHistory, action: \.answerHistory) {
      AnswerHistory(client: client.answerHistory)
    }
    Scope(state: \.modelLibrary, action: \.modelLibrary) {
      ModelLibrary(client: client.modelLibrary)
    }
    Scope(state: \.answerUsage, action: \.answerUsage) {
      AnswerUsage(read: client.answerUsage)
    }
    Scope(state: \.diagnosticLogs, action: \.diagnosticLogs) {
      DiagnosticLogs(read: client.diagnosticLogs)
    }
    Scope(state: \.answerRequests, action: \.answerRequests) {
      DiagnosticLogs { [client] _, _, beforeID in
        try await client.diagnosticLogs("api_answer", nil, beforeID)
      }
    }
    Scope(state: \.traceInspector, action: \.traceInspector) {
      TraceInspector(readSavedAnswer: client.answerHistory.byTrace, read: client.readTrace)
    }
    Scope(state: \.settingsForm, action: \.settingsForm) {
      SettingsForm(
        loadEmbeddingModels: client.loadEmbeddingModels,
        selectEmbeddingModel: client.selectEmbeddingModel, save: client.save)
    }
    Reduce { state, action in
      switch action {
      case .destinationChanged(let destination):
        let wasShowingUsage = state.showsReadyUsage
        let wasShowingLogs = state.showsReadyLogs
        state.destination = destination
        return .merge(
          destination == .overview ? .none : .cancel(id: Cancellation.overview),
          diagnosticLogVisibilityChanged(from: wasShowingLogs, to: state.showsReadyLogs),
          usageVisibilityChanged(from: wasShowingUsage, to: state.showsReadyUsage))
      case .observeOverview:
        guard state.destination == .overview, state.connection == .ready else { return .none }
        return observeOverview()
      case .overviewStatusLoaded(let status):
        if state.destination == .overview, state.connection == .ready {
          state.snapshot?.status = status
        }
        return .none
      case .observeBackend:
        return .run { [client] send in
          for await reason in await client.exits() {
            await send(.backendExited(reason))
          }
        }
      case .refresh, .restart:
        let wasShowingUsage = state.showsReadyUsage
        let wasShowingLogs = state.showsReadyLogs
        state.connection = .connecting
        let operation = if case .restart = action { client.restart } else { client.snapshot }
        let refresh: EffectOf<Self> = .run { send in
          do {
            let snapshot = try await operation()
            try Task.checkCancellation()
            await send(.loaded(snapshot))
          } catch is CancellationError {
          } catch BackendConnectionError.dataDirectoryInUse {
            await send(.backendExited(.dataDirectoryInUse))
          } catch {
            await send(.loadFailed(error.localizedDescription))
          }
        }
        .cancellable(id: Cancellation.refresh, cancelInFlight: true)
        return .merge(
          .cancel(id: Cancellation.overview),
          diagnosticLogVisibilityChanged(from: wasShowingLogs, to: false),
          usageVisibilityChanged(from: wasShowingUsage, to: false), refresh,
          discardInspectorAnswerIfNeeded(state))
      case .loaded(let snapshot):
        state.snapshot = snapshot
        state.settingsForm.refresh(snapshot.settings)
        state.connection = .ready
        return .merge(
          diagnosticLogVisibilityChanged(from: false, to: state.showsReadyLogs),
          usageVisibilityChanged(from: false, to: state.showsReadyUsage))
      case .backendExited(let reason):
        let wasShowingUsage = state.showsReadyUsage
        let wasShowingLogs = state.showsReadyLogs
        state.answerHistory.discardCachedEntries()
        state.connection =
          reason == .dataDirectoryInUse
          ? .dataDirectoryInUse : .unavailable(reason.localizedDescription)
        return .merge(
          diagnosticLogVisibilityChanged(from: wasShowingLogs, to: false),
          usageVisibilityChanged(from: wasShowingUsage, to: false),
          .cancel(id: Cancellation.overview),
          .cancel(id: Cancellation.refresh),
          state.traceInspector.traceID == nil ? .none : .send(.traceInspector(.close)))
      case .loadFailed(let message):
        state.connection = .unavailable(message)
        state.answerHistory.discardCachedEntries()
        return .merge(.cancel(id: Cancellation.overview), discardInspectorAnswerIfNeeded(state))
      case .github(.result(.purged(let id, _), _)):
        state.answerHistory.discardCachedEntries()
        return .merge(
          .send(.answers(.repositoryDataPurged(id))), discardInspectorAnswerIfNeeded(state))
      case .answerHistory(.cleared):
        state.answers.discardSessionQuestions()
        return discardInspectorAnswerIfNeeded(state)
      case .answerHistory(.clear), .answerHistory(.settingsSaved),
        .answerHistory(.settingsLoaded):
        return discardInspectorAnswerIfNeeded(state)
      case .settingsForm, .github, .answers, .answerHistory, .modelLibrary, .diagnosticLogs,
        .answerRequests,
        .answerUsage,
        .traceInspector:
        return .none
      case .diagnosticTabChanged(let tab):
        let wasShowingUsage = state.showsReadyUsage
        let wasShowingLogs = state.showsReadyLogs
        state.diagnosticTab = tab
        return .merge(
          diagnosticLogVisibilityChanged(from: wasShowingLogs, to: state.showsReadyLogs),
          usageVisibilityChanged(from: wasShowingUsage, to: state.showsReadyUsage))
      case .readTelemetry:
        guard state.connection == .ready, !state.telemetryBusy else { return .none }
        return .run { [client] send in
          await send(.telemetryRead(try await client.readTelemetry()))
        } catch: { _, _ in
        }
      case .telemetryRead(let snapshot):
        if !state.telemetryBusy, state.connection == .ready {
          state.snapshot?.telemetry = snapshot
        }
        return .none
      case .telemetryCommand(let command):
        guard !state.telemetryBusy else { return .none }
        state.telemetryFeedback = nil
        state.telemetryBusy = true
        return .run { [client] send in
          do {
            await send(.telemetryUpdated(try await client.telemetry(command)))
          } catch {
            await send(.telemetryFailed(error.localizedDescription))
          }
        }
      case .telemetryUpdated(let snapshot):
        state.telemetryBusy = false
        state.snapshot?.telemetry = snapshot
        if state.showsReadyUsage { return .send(.answerUsage(.refresh)) }
        return state.showsReadyLogs ? .send(.diagnosticLogs(.refresh)) : .none
      case .telemetryFailed(let message):
        state.telemetryBusy = false
        state.telemetryFeedback = message
        return .none

      }
    }
  }

  private func observeOverview() -> EffectOf<Self> {
    let pollingClock = clock
    return .run { [readStatus = client.readStatus] send in
      while !Task.isCancelled {
        if let status = try? await readStatus() {
          try Task.checkCancellation()
          await send(.overviewStatusLoaded(status))
        }
        try await pollingClock.sleep(for: .seconds(3))
      }
    }
    .cancellable(id: Cancellation.overview, cancelInFlight: true)
  }

  private func discardInspectorAnswerIfNeeded(_ state: State) -> EffectOf<Self> {
    guard state.traceInspector.savedAnswer != nil || state.traceInspector.loadingSavedAnswer else {
      return .none
    }
    return .send(.traceInspector(.discardSavedAnswer))
  }

  private func usageVisibilityChanged(from wasShowing: Bool, to isShowing: Bool) -> EffectOf<Self> {
    guard wasShowing != isShowing else { return .none }
    return .send(.answerUsage(isShowing ? .refresh : .reset))
  }

  private func diagnosticLogVisibilityChanged(from wasShowing: Bool, to isShowing: Bool)
    -> EffectOf<Self>
  {
    guard wasShowing != isShowing else { return .none }
    return .send(.diagnosticLogs(isShowing ? .refresh : .cancel))
  }
}
