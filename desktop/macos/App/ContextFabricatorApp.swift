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

  var body: some Scene {
    Window("Context Fabricator", id: "dashboard") {
      RootView(supervisor: delegate.supervisor)
    }
    .defaultSize(width: 1120, height: 720)
    .commands {
      SidebarCommands()
      GitHubCommands()
    }
  }
}

private struct RootView: View {
  @State private var store: StoreOf<Dashboard>
  private let inferenceDirectory: URL

  init(supervisor: BackendSupervisor) {
    inferenceDirectory = supervisor.inferenceDirectory
    let client = DashboardClient(
      snapshot: { try await Self.snapshot(supervisor.connect()) },
      save: { settings in try await supervisor.connect(startIfNeeded: false).save(settings) },
      restart: { try await Self.snapshot(supervisor.restart()) },
      exits: { await supervisor.exits() },
      telemetry: { command in
        try await supervisor.connect(startIfNeeded: false).controlTelemetry(command)
      },
      answers: AnswersClient(
        ask: { question, id in
          try await supervisor.connect(startIfNeeded: false).askWithTrace(question, id: id)
        },
        cancel: { id in try await supervisor.connect(startIfNeeded: false).cancelAnswer(id) },
        loadModels: { try await supervisor.connect(startIfNeeded: false).generationModels() },
        selectModel: { profile in
          try await supervisor.connect(startIfNeeded: false).selectGenerationModel(profile)
        }),
      github: { command in
        try await command.perform(using: supervisor.connect(startIfNeeded: false))
      },
      modelLibrary: ModelLibraryClient(
        catalog: { try await supervisor.connect(startIfNeeded: false).modelDownloads() },
        install: { profile in
          try await supervisor.connect(startIfNeeded: false).installModel(profile)
        }),
      diagnosticLogs: { query, level, beforeId in
        try await supervisor.connect(startIfNeeded: false)
          .diagnosticLogs(query: query, level: level, beforeId: beforeId)
      },
      readTrace: { id, before in
        try await supervisor.connect(startIfNeeded: false).diagnosticTrace(id, beforeId: before)
      }
    )
    self._store = State(
      initialValue: Store(initialState: Dashboard.State()) { Dashboard(client: client) })
  }

  var body: some View {
    DashboardView(store: store, inferenceDirectory: inferenceDirectory)
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
