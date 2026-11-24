import AppKit
import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
  let supervisor: BackendSupervisor = {
    let installedDirectory = URL.applicationSupportDirectory.appending(path: "ContextFabricator")
    let bundledHelper = Bundle.main.url(forAuxiliaryExecutable: "ctxfab-server")
    #if DEBUG
      let environment = ProcessInfo.processInfo.environment
      let directory =
        environment["CTXFAB_TEST_DATA_DIRECTORY"].map {
          URL(filePath: $0, directoryHint: .isDirectory)
        } ?? installedDirectory
      let helper = environment["CTXFAB_TEST_BACKEND_DISABLED"] == "1" ? nil : bundledHelper
      let fixtureArguments =
        if let fixture = environment["CTXFAB_TEST_CREDENTIAL_FIXTURE"],
          let correlation = environment["CTXFAB_TEST_CREDENTIAL_CORRELATION"]
        {
          [fixture, correlation]
        } else if environment["CTXFAB_TEST_DATA_DIRECTORY"] != nil,
          let port = environment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"]
        {
          [
            environment["CTXFAB_TEST_GITHUB_SCHEDULED"] == "1"
              ? "--github-scheduled-fixture" : "--github-fixture",
            port,
          ]
        } else {
          [String]()
        }
    #else
      let directory = installedDirectory
      let helper = bundledHelper
      let fixtureArguments: [String] = []
    #endif
    return BackendSupervisor(
      directory: directory, helper: helper, fixtureArguments: fixtureArguments)
  }()

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    false
  }

  func applicationWillTerminate(_ notification: Notification) {
    supervisor.stop()
  }
}

@main
struct ContextFabricatorApp: App {
  @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
  @AppStorage(AppLanguage.preferenceKey) private var appLanguage: AppLanguage = .system

  init() {
    #if DEBUG
      if ProcessInfo.processInfo.environment["CTXFAB_TEST_DATA_DIRECTORY"] != nil {
        _appLanguage = AppStorage(
          wrappedValue: .system, AppLanguage.preferenceKey,
          store: UserDefaults(suiteName: "org.contextfabricator.uitests.preferences"))
      }
    #endif
  }

  var body: some Scene {
    let locale = appLanguage.locale()
    Window("Context Fabricator", id: "dashboard") {
      RootView(supervisor: delegate.supervisor, appLanguage: $appLanguage)
        .frame(minWidth: 900, minHeight: 660)
        .environment(\.locale, locale)
    }
    .defaultSize(width: 1280, height: 820)
    .windowResizability(.contentMinSize)
    .commands {
      SidebarCommands()
      GitHubCommands(locale: locale)
    }
  }
}

private struct RootView: View {
  @State private var store: StoreOf<Dashboard>
  private let inferenceDirectory: URL
  private let supervisor: BackendSupervisor
  @Binding var appLanguage: AppLanguage

  init(supervisor: BackendSupervisor, appLanguage: Binding<AppLanguage>) {
    self.supervisor = supervisor
    inferenceDirectory = supervisor.inferenceDirectory
    _appLanguage = appLanguage
    let client = DashboardClient(
      snapshot: { try await Self.snapshot(supervisor.connect()) },
      save: { settings in try await supervisor.connect(startIfNeeded: false).save(settings) },
      restart: { try await Self.snapshot(supervisor.restart()) },
      exits: { await supervisor.exits() },
      loadEmbeddingModels: {
        try await supervisor.connect(startIfNeeded: false).embeddingModels()
      },
      selectEmbeddingModel: { profile in
        try await supervisor.connect(startIfNeeded: false).selectEmbeddingModel(profile)
      },
      readStatus: { try await supervisor.connect(startIfNeeded: false).status() },
      telemetry: { command in
        try await supervisor.connect(startIfNeeded: false).controlTelemetry(command)
      },
      answers: Self.answersClient(supervisor),
      answerHistory: Self.answerHistoryClient(supervisor),
      github: { command in
        try await command.perform(using: supervisor.connect(startIfNeeded: false))
      },
      modelLibrary: Self.modelLibraryClient(supervisor),
      answerUsage: {
        try await supervisor.connect(startIfNeeded: false).answerUsage()
      },
      diagnosticLogs: { query, level, beforeId in
        try await supervisor.connect(startIfNeeded: false)
          .diagnosticLogs(query: query, level: level, beforeId: beforeId)
      },
      readTrace: { id, before in
        try await supervisor.connect(startIfNeeded: false).diagnosticTrace(id, beforeId: before)
      },
      readTelemetry: {
        try await supervisor.connect(startIfNeeded: false).telemetry()
      }
    )
    self._store = State(
      initialValue: Store(initialState: Dashboard.State()) { Dashboard(client: client) })
  }

  var body: some View {
    DashboardView(
      store: store, inferenceDirectory: inferenceDirectory, supervisor: supervisor,
      appLanguage: $appLanguage)
  }

  private static func answerHistoryClient(_ supervisor: BackendSupervisor) -> AnswerHistoryClient {
    AnswerHistoryClient(
      loadSettings: {
        try await supervisor.connect(startIfNeeded: false).answerHistorySettings()
      },
      saveSettings: {
        try await supervisor.connect(startIfNeeded: false).saveAnswerHistorySettings($0)
      },
      list: { try await supervisor.connect(startIfNeeded: false).answerHistory(beforeId: $0) },
      detail: { try await supervisor.connect(startIfNeeded: false).answerHistoryEntry(id: $0) },
      byTrace: {
        try await supervisor.connect(startIfNeeded: false).answerHistoryEntry(traceId: $0)
      },
      clear: { try await supervisor.connect(startIfNeeded: false).clearAnswerHistory() })
  }

  private static func modelLibraryClient(_ supervisor: BackendSupervisor) -> ModelLibraryClient {
    ModelLibraryClient(
      catalog: { try await supervisor.connect(startIfNeeded: false).modelDownloads() },
      install: { profile in
        try await supervisor.connect(startIfNeeded: false).installModel(profile)
      },
      delete: { profile in
        try await supervisor.connect(startIfNeeded: false).deleteModel(profile)
      },
      pause: { profile in
        try await supervisor.connect(startIfNeeded: false).pauseModel(profile)
      },
      resume: { profile in
        try await supervisor.connect(startIfNeeded: false).resumeModel(profile)
      },
      discard: { profile in
        try await supervisor.connect(startIfNeeded: false).discardModelPartial(profile)
      },
      removalPlan: { profile in
        try await supervisor.connect(startIfNeeded: false).modelRemovalPlan(profile)
      },
      deleteConfirmed: { profile, identity in
        try await supervisor.connect(startIfNeeded: false).deleteModel(profile, identity: identity)
      },
      indexStatus: { try await supervisor.connect(startIfNeeded: false).embeddingIndex() },
      setIndexEnabled: { enabled in
        try await supervisor.connect(startIfNeeded: false).setEmbeddingIndexEnabled(enabled)
      },
      deleteIndex: { try await supervisor.connect(startIfNeeded: false).deleteEmbeddingIndex() },
      storagePlan: { try await supervisor.connect(startIfNeeded: false).inferenceStoragePlan() },
      runtimeStatus: { try await supervisor.connect(startIfNeeded: false).runtimeStatus() },
      installRuntime: { path in
        try await supervisor.connect(startIfNeeded: false).installRuntime(archivePath: path)
      },
      setup: Self.inferenceSetupClient(supervisor),
      hub: HubCatalogClient(
        search: { query, kind in
          try await supervisor.connect(startIfNeeded: false).searchHubCatalog(query, kind: kind)
        },
        variants: { repository, kind in
          try await supervisor.connect(startIfNeeded: false)
            .hubCatalogVariants(repository, kind: kind)
        },
        install: { repository, file, kind in
          try await supervisor.connect(startIfNeeded: false).installHubModel(
            repository, variant: file, kind: kind)
        }))
  }

  private static func inferenceSetupClient(_ supervisor: BackendSupervisor) -> InferenceSetupClient
  {
    InferenceSetupClient(
      plan: { try await supervisor.connect(startIfNeeded: false).inferenceSetupPlan($0) },
      start: { selection, fingerprint, operationID, archivePath in
        try await supervisor.connect(startIfNeeded: false).startInferenceSetup(
          selection: selection, fingerprint: fingerprint, operationID: operationID,
          archivePath: archivePath)
      },
      status: { try await supervisor.connect(startIfNeeded: false).inferenceSetupStatus() },
      provideArchive: { operationID, path in
        try await supervisor.connect(startIfNeeded: false).resumeInferenceSetup(
          operationID: operationID, archivePath: path)
      },
      cancelArchive: { operationID in
        try await supervisor.connect(startIfNeeded: false).cancelInferenceSetupArchive(
          operationID: operationID)
      }
    )
  }

  private static func answersClient(_ supervisor: BackendSupervisor) -> AnswersClient {
    AnswersClient(
      ask: { question, id in
        try await supervisor.connect(startIfNeeded: false).askWithTrace(question, id: id)
      },
      askCode: { question, id, selection in
        try await supervisor.connect(startIfNeeded: false).askWithTrace(
          question, id: id, repositoryId: selection.repositoryId, branch: selection.branch)
      },
      askHistory: { question, id, selection in
        try await supervisor.connect(startIfNeeded: false).askWithTrace(
          question, id: id, repositoryId: selection.repositoryId, branch: selection.branch,
          historyCommit: selection.child)
      },
      askActions: { question, id, selection in
        try await supervisor.connect(startIfNeeded: false).askWithTrace(
          question, id: id, repositoryId: selection.repositoryId, branch: selection.branch,
          actionsRunId: selection.runId, actionsRunAttempt: selection.attempt)
      },
      loadActions: { id in
        try await supervisor.connect(startIfNeeded: false).gitHubActions(id: id)
      },
      loadRepositories: {
        try await supervisor.connect(startIfNeeded: false).githubState().repositories
      },
      refreshHistory: { id in
        try await supervisor.connect(startIfNeeded: false).refreshGitHubHistory(id: id)
      },
      materializeHistory: { selection in
        try await supervisor.connect(startIfNeeded: false).materializeGitHubHistory(
          id: selection.repositoryId, child: selection.child)
      },
      cancel: { id in try await supervisor.connect(startIfNeeded: false).cancelAnswer(id) },
      loadModels: { try await supervisor.connect(startIfNeeded: false).generationModels() },
      selectModel: { profile in
        try await supervisor.connect(startIfNeeded: false).selectGenerationModel(profile)
      })
  }

  private static func snapshot(_ session: SessionClient) async throws -> DashboardSnapshot {
    async let status = session.status()
    async let settings = session.settings()
    async let diagnostics = session.diagnostics()
    async let telemetry = try? session.telemetry()
    return try await DashboardSnapshot(
      status: status, settings: settings, diagnostics: diagnostics, telemetry: telemetry)
  }
}
