import ComposableArchitecture
import LocalAPI

public struct DashboardSnapshot: Equatable, Sendable {
  public let status: Components.Schemas.Status
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
  let save: @Sendable (Components.Schemas.Settings) async throws -> Components.Schemas.Settings
  let restart: @Sendable () async throws -> DashboardSnapshot
  let exits: @Sendable () async -> AsyncStream<BackendConnectionError>
  let github: @Sendable (GitHubCommand) async throws -> GitHubResult
  let answers: AnswersClient
  let modelLibrary: ModelLibraryClient
  let diagnosticLogs: DiagnosticLogs.Read
  let readTrace: TraceInspector.Read
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
    telemetry:
      @escaping @Sendable (Components.Schemas.TelemetryControl.commandPayload) async throws ->
      Components.Schemas.TelemetrySnapshot =
      { _ in throw BackendConnectionError.helperExited },
    answers: AnswersClient = .init(),
    github: @escaping @Sendable (GitHubCommand) async throws -> GitHubResult =
      { _ in throw BackendConnectionError.helperExited },
    modelLibrary: ModelLibraryClient = .init(
      catalog: { [] },
      install: { _ in
        throw BackendConnectionError.helperExited
      }),
    diagnosticLogs: @escaping DiagnosticLogs.Read = { _, _, _ in .init(records: []) },
    readTrace: @escaping TraceInspector.Read = { _, _ in throw BackendConnectionError.helperExited }
  ) {
    self.snapshot = snapshot
    self.save = save
    self.restart = restart
    self.exits = exits
    self.telemetry = telemetry
    self.github = github
    self.answers = answers
    self.modelLibrary = modelLibrary
    self.diagnosticLogs = diagnosticLogs
    self.readTrace = readTrace
  }
}

@Reducer
public struct Dashboard {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var snapshot: DashboardSnapshot?
    public internal(set) var github = GitHubConnector.State()
    public internal(set) var answers = Answers.State()
    public internal(set) var modelLibrary = ModelLibrary.State()
    public internal(set) var diagnosticLogs = DiagnosticLogs.State()
    public internal(set) var traceInspector = TraceInspector.State()
    public internal(set) var settingsForm: SettingsForm.State
    public internal(set) var connection: Connection = .connecting
    public internal(set) var destination: Destination = .overview
    public internal(set) var telemetryFeedback: String?
    public internal(set) var telemetryBusy = false
    public internal(set) var diagnosticTab: DiagnosticTab = .logs
    fileprivate var showsReadyLogs: Bool {
      destination == .diagnostics && diagnosticTab == .logs && connection == .ready
    }

    public init(settings: Components.Schemas.Settings? = nil) {
      self.settingsForm = .init(confirmed: settings)
    }
  }

  public enum Connection: Equatable {
    case connecting
    case ready
    case unavailable(String)
  }

  public enum Destination: String, CaseIterable, Sendable {
    case overview = "Overview"
    case answers = "Answers"
    case connectors = "Connectors"
    case diagnostics = "Diagnostics"
    case settings = "Settings"
  }

  public enum DiagnosticTab: String, CaseIterable, Sendable {
    case logs = "Logs"
    case metrics = "Metrics"
    case traces = "Traces"
    case storage = "Storage"
  }

  public enum Action {
    case github(GitHubConnector.Action)
    case answers(Answers.Action)
    case modelLibrary(ModelLibrary.Action)
    case diagnosticLogs(DiagnosticLogs.Action)
    case traceInspector(TraceInspector.Action)
    case destinationChanged(Destination)
    case observeBackend
    case refresh
    case restart
    case loaded(DashboardSnapshot)
    case backendExited(BackendConnectionError)
    case loadFailed(String)
    case settingsForm(SettingsForm.Action)
    case diagnosticTabChanged(DiagnosticTab)
    case telemetryCommand(Components.Schemas.TelemetryControl.commandPayload)
    case telemetryUpdated(Components.Schemas.TelemetrySnapshot)
    case telemetryFailed(String)

  }

  private enum Cancellation: Hashable {
    case refresh
  }

  let client: DashboardClient

  public init(client: DashboardClient) {
    self.client = client
  }

  public var body: some ReducerOf<Self> {
    Scope(state: \.github, action: \.github) { GitHubConnector(perform: client.github) }
    Scope(state: \.answers, action: \.answers) { Answers(client: client.answers) }
    Scope(state: \.modelLibrary, action: \.modelLibrary) {
      ModelLibrary(client: client.modelLibrary)
    }
    Scope(state: \.diagnosticLogs, action: \.diagnosticLogs) {
      DiagnosticLogs(read: client.diagnosticLogs)
    }
    Scope(state: \.traceInspector, action: \.traceInspector) {
      TraceInspector(read: client.readTrace)
    }
    Scope(state: \.settingsForm, action: \.settingsForm) {
      SettingsForm(save: client.save)
    }
    Reduce { state, action in
      switch action {
      case .destinationChanged(let destination):
        let wasShowingLogs = state.showsReadyLogs
        state.destination = destination
        return diagnosticLogVisibilityChanged(from: wasShowingLogs, to: state.showsReadyLogs)
      case .observeBackend:
        return .run { [client] send in
          for await reason in await client.exits() {
            await send(.backendExited(reason))
          }
        }
      case .refresh, .restart:
        let wasShowingLogs = state.showsReadyLogs
        state.connection = .connecting
        let operation = if case .restart = action { client.restart } else { client.snapshot }
        let refresh: EffectOf<Self> = .run { send in
          do {
            let snapshot = try await operation()
            try Task.checkCancellation()
            await send(.loaded(snapshot))
          } catch is CancellationError {
          } catch {
            await send(.loadFailed(error.localizedDescription))
          }
        }
        .cancellable(id: Cancellation.refresh, cancelInFlight: true)
        return .merge(diagnosticLogVisibilityChanged(from: wasShowingLogs, to: false), refresh)
      case .loaded(let snapshot):
        state.snapshot = snapshot
        state.settingsForm.refresh(snapshot.settings)
        state.connection = .ready
        return diagnosticLogVisibilityChanged(from: false, to: state.showsReadyLogs)
      case .backendExited(let reason):
        let wasShowingLogs = state.showsReadyLogs
        state.connection = .unavailable(reason.localizedDescription)
        return .merge(
          diagnosticLogVisibilityChanged(from: wasShowingLogs, to: false),
          .cancel(id: Cancellation.refresh))
      case .loadFailed(let message):
        state.connection = .unavailable(message)
        return .none
      case .settingsForm, .github, .answers, .modelLibrary, .diagnosticLogs, .traceInspector:
        return .none
      case .diagnosticTabChanged(let tab):
        let wasShowingLogs = state.showsReadyLogs
        state.diagnosticTab = tab
        return diagnosticLogVisibilityChanged(from: wasShowingLogs, to: state.showsReadyLogs)
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
        return state.showsReadyLogs ? .send(.diagnosticLogs(.refresh)) : .none
      case .telemetryFailed(let message):
        state.telemetryBusy = false
        state.telemetryFeedback = message
        return .none

      }
    }
  }

  private func diagnosticLogVisibilityChanged(from wasShowing: Bool, to isShowing: Bool)
    -> EffectOf<Self>
  {
    guard wasShowing != isShowing else { return .none }
    return .send(.diagnosticLogs(isShowing ? .refresh : .cancel))
  }
}
