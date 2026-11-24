import ComposableArchitecture
import Foundation
import LocalAPI

public struct ModelLibraryClient: Sendable {
  public typealias Catalog = @Sendable () async throws -> [Components.Schemas.ModelDownload]
  public typealias Install = @Sendable (String) async throws -> Void

  let catalog: Catalog
  let install: Install
  let delete: Install
  let pause: Install
  let resume: Install
  let discard: Install
  let removalPlan: (@Sendable (String) async throws -> Components.Schemas.ModelRemovalPlan)?
  let deleteConfirmed: (@Sendable (String, String) async throws -> Void)?
  let indexStatus: (@Sendable () async throws -> Components.Schemas.EmbeddingIndexState)?
  let setIndexEnabled: (@Sendable (Bool) async throws -> Components.Schemas.EmbeddingIndexState)?
  let deleteIndex: (@Sendable () async throws -> Components.Schemas.EmbeddingIndexDeletion)?
  let storagePlan: (@Sendable () async throws -> Components.Schemas.InferenceStoragePlan)?
  let runtimeStatus: (@Sendable () async throws -> Components.Schemas.RuntimeStatus)?
  let installRuntime: (@Sendable (String) async throws -> Components.Schemas.RuntimeStatus)?
  let downloadRuntime: (@Sendable () async throws -> Components.Schemas.RuntimeStatus)?
  let setup: InferenceSetupClient?
  let hub: HubCatalogClient

  @preconcurrency public init(
    catalog: @escaping Catalog,
    install: @escaping Install,
    delete: @escaping Install = { _ in throw BackendConnectionError.helperExited },
    pause: @escaping Install = { _ in throw BackendConnectionError.helperExited },
    resume: @escaping Install = { _ in throw BackendConnectionError.helperExited },
    discard: @escaping Install = { _ in throw BackendConnectionError.helperExited },
    removalPlan: (@Sendable (String) async throws -> Components.Schemas.ModelRemovalPlan)? = nil,
    deleteConfirmed: (@Sendable (String, String) async throws -> Void)? = nil,
    indexStatus: (@Sendable () async throws -> Components.Schemas.EmbeddingIndexState)? = nil,
    setIndexEnabled: (@Sendable (Bool) async throws -> Components.Schemas.EmbeddingIndexState)? =
      nil,
    deleteIndex: (@Sendable () async throws -> Components.Schemas.EmbeddingIndexDeletion)? = nil,
    storagePlan: (@Sendable () async throws -> Components.Schemas.InferenceStoragePlan)? = nil,
    runtimeStatus: (@Sendable () async throws -> Components.Schemas.RuntimeStatus)? = nil,
    installRuntime: (@Sendable (String) async throws -> Components.Schemas.RuntimeStatus)? = nil,
    downloadRuntime: (@Sendable () async throws -> Components.Schemas.RuntimeStatus)? = nil,
    setup: InferenceSetupClient? = nil,
    hub: HubCatalogClient = .init()
  ) {
    self.catalog = catalog
    self.install = install
    self.delete = delete
    self.pause = pause
    self.resume = resume
    self.discard = discard
    self.removalPlan = removalPlan
    self.deleteConfirmed = deleteConfirmed
    self.indexStatus = indexStatus
    self.setIndexEnabled = setIndexEnabled
    self.deleteIndex = deleteIndex
    self.storagePlan = storagePlan
    self.runtimeStatus = runtimeStatus
    self.installRuntime = installRuntime
    self.downloadRuntime = downloadRuntime
    self.setup = setup
    self.hub = hub
  }
}

@Reducer
public struct ModelLibrary {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var setup = InferenceSetup.State()
    public internal(set) var removalPlan: Components.Schemas.ModelRemovalPlan?
    public internal(set) var index: Components.Schemas.EmbeddingIndexState?
    public internal(set) var indexPending = false
    public internal(set) var indexFeedback: String?
    public internal(set) var indexRemoval: Components.Schemas.EmbeddingIndexDeletion?
    public internal(set) var indexStatusFeedback: String?
    public internal(set) var storagePlan: Components.Schemas.InferenceStoragePlan?
    public internal(set) var storageFeedback: String?
    public internal(set) var runtime: Components.Schemas.RuntimeStatus?
    public internal(set) var runtimeFeedback: String?
    public internal(set) var runtimeStatusFeedback: String?
    public internal(set) var runtimeRequestPending = false
    public internal(set) var profiles: [Components.Schemas.ModelDownload]?
    public internal(set) var pendingProfile: String?
    public internal(set) var feedback: String?
    public internal(set) var feedbackProfile: String?
    public internal(set) var catalogFeedback: String?
    public internal(set) var showingDiscovery = false
    public internal(set) var hub = HubCatalog.State()
    var isObserving = false
    var acceptedProfile: String?
  }

  public enum Action {
    case observe
    case setup(InferenceSetup.Action)
    case indexLoaded(Components.Schemas.EmbeddingIndexState)
    case indexFailed(String)
    case setIndexEnabled(Bool)
    case deleteIndex
    case indexUpdated(Components.Schemas.EmbeddingIndexState)
    case indexDeleted(Components.Schemas.EmbeddingIndexDeletion)
    case indexRequestFailed(String)
    case storageLoaded(Components.Schemas.InferenceStoragePlan)
    case storageFailed(String)
    case runtimeLoaded(Components.Schemas.RuntimeStatus)
    case runtimeFailed(String)
    case runtimeInstallCompleted(Components.Schemas.RuntimeStatus)
    case runtimeInstallFailed(String)
    case installRuntime(String)
    case downloadRuntime
    case openDiscovery
    case hub(HubCatalog.Action)
    case stopObserving
    case install(String)
    case delete(String)
    case prepareRemoval(String)
    case removalPlanLoaded(Components.Schemas.ModelRemovalPlan)
    case cancelRemoval
    case confirmRemoval
    case removalConflict(String)
    case pause(String)
    case resume(String)
    case discardPartial(String)
    case catalogLoaded([Components.Schemas.ModelDownload])
    case catalogFailed(String)
    case installationAccepted(String)
    case installationRejected(String, String)
    case deletionCompleted(String)
    case deletionRejected(String, String)
  }

  private enum Cancellation: Hashable { case observation, runtimeObservation }

  private let client: ModelLibraryClient
  @Dependency(\.continuousClock) private var clock

  public init(client: ModelLibraryClient) { self.client = client }

  public var body: some ReducerOf<Self> {
    Scope(state: \.hub, action: \.hub) { HubCatalog(client: client.hub) }
    Scope(state: \.setup, action: \.setup) {
      InferenceSetup(client: client.setup ?? .unavailable)
    }
    Reduce { state, action in
      switch action {
      case .setup:
        return .none
      case .openDiscovery:
        state.showingDiscovery = true
        return .none
      case .hub(.close):
        state.showingDiscovery = false
        return .none
      case .hub(.downloadAccepted):
        state.showingDiscovery = false
        return state.isObserving ? observe() : .none
      case .hub:
        return .none
      case .observe:
        state.isObserving = true
        state.catalogFeedback = nil
        return .merge(
          observe(), observeRuntime(), client.setup == nil ? .none : .send(.setup(.observe)))
      case .indexLoaded(let index):
        if !state.indexPending {
          state.index = index
          state.indexStatusFeedback = nil
        }
        return .none
      case .indexFailed(let message):
        if !state.indexPending { state.indexStatusFeedback = message }
        return .none
      case .setIndexEnabled(let enabled):
        guard !state.indexPending, let index = state.index, index.enabled != enabled,
          let update = client.setIndexEnabled
        else { return .none }
        state.indexPending = true
        state.indexFeedback = nil
        state.indexRemoval = nil
        return .run { send in
          do { await send(.indexUpdated(try await update(enabled))) } catch {
            await send(.indexRequestFailed(error.localizedDescription))
          }
        }
      case .deleteIndex:
        guard !state.indexPending, state.index != nil, let delete = client.deleteIndex else {
          return .none
        }
        state.indexPending = true
        state.indexFeedback = nil
        state.indexRemoval = nil
        return .run { send in
          do { await send(.indexDeleted(try await delete())) } catch {
            await send(.indexRequestFailed(error.localizedDescription))
          }
        }
      case .indexUpdated(let index):
        state.indexPending = false
        state.index = index
        return .none
      case .indexDeleted(let deletion):
        state.indexPending = false
        state.index = deletion.value1
        state.indexRemoval = deletion
        return .none
      case .indexRequestFailed(let message):
        state.indexPending = false
        state.indexFeedback = message
        return .none
      case .storageLoaded(let plan):
        state.storagePlan = plan
        state.storageFeedback = nil
        return .none
      case .storageFailed(let message):
        state.storagePlan = nil
        state.storageFeedback = message
        return .none
      case .runtimeLoaded(let runtime):
        state.runtime = runtime
        state.runtimeStatusFeedback = nil
        return .none
      case .runtimeFailed(let message):
        state.runtimeStatusFeedback = message
        return .none
      case .runtimeInstallCompleted(let runtime):
        state.runtimeRequestPending = false
        state.runtime = runtime
        state.runtimeFeedback = nil
        return state.isObserving ? observeRuntime() : .none
      case .runtimeInstallFailed(let message):
        state.runtimeRequestPending = false
        state.runtimeFeedback = message
        return state.isObserving ? observeRuntime() : .none
      case .installRuntime(let path):
        guard !state.runtimeRequestPending, let runtime = state.runtime,
          runtime.state == .availableForInstall || runtime.state == .failed,
          let install = client.installRuntime
        else { return .none }
        state.runtimeRequestPending = true
        state.runtimeFeedback = nil
        let request: EffectOf<Self> = .run { send in
          do { await send(.runtimeInstallCompleted(try await install(path))) } catch {
            await send(.runtimeInstallFailed(error.localizedDescription))
          }
        }
        return .merge(.cancel(id: Cancellation.runtimeObservation), request)
      case .downloadRuntime:
        guard !state.runtimeRequestPending, let runtime = state.runtime,
          runtime.canDownload == true,
          runtime.state == .availableForInstall || runtime.state == .failed,
          let download = client.downloadRuntime
        else { return .none }
        state.runtimeRequestPending = true
        state.runtimeFeedback = nil
        let request: EffectOf<Self> = .run { send in
          do { await send(.runtimeInstallCompleted(try await download())) } catch {
            await send(.runtimeInstallFailed(error.localizedDescription))
          }
        }
        return .merge(.cancel(id: Cancellation.runtimeObservation), request)
      case .stopObserving:
        state.isObserving = false
        state.profiles = nil
        state.runtime = nil
        state.storagePlan = nil
        state.storageFeedback = nil
        state.index = nil
        state.removalPlan = nil
        state.indexFeedback = nil
        state.indexRemoval = nil
        state.indexStatusFeedback = nil
        if state.acceptedProfile != nil {
          state.pendingProfile = nil
          state.acceptedProfile = nil
        }
        return .merge(
          .cancel(id: Cancellation.observation), .cancel(id: Cancellation.runtimeObservation),
          client.setup == nil ? .none : .send(.setup(.stop)))
      case .install(let profile):
        guard state.pendingProfile == nil,
          let model = state.profiles?.first(where: {
            $0.profile == profile
          }), model.state == .notInstalled || model.state == .failed
        else { return .none }
        state.pendingProfile = profile
        state.feedback = nil
        state.feedbackProfile = nil
        return install(profile)
      case .pause(let profile):
        guard state.pendingProfile == nil,
          state.profiles?.contains(where: { $0.profile == profile && $0.state == .installing })
            == true
        else { return .none }
        state.pendingProfile = profile
        state.feedback = nil
        state.feedbackProfile = nil
        return control(profile, action: client.pause)
      case .resume(let profile):
        guard state.pendingProfile == nil,
          state.profiles?.contains(where: {
            $0.profile == profile && ($0.state == .paused || $0.state == .interrupted)
          }) == true
        else { return .none }
        state.pendingProfile = profile
        state.feedback = nil
        state.feedbackProfile = nil
        return control(profile, action: client.resume)
      case .discardPartial(let profile):
        guard state.pendingProfile == nil,
          state.profiles?.contains(where: {
            $0.profile == profile && ($0.state == .paused || $0.state == .interrupted)
          }) == true
        else { return .none }
        state.pendingProfile = profile
        state.feedback = nil
        state.feedbackProfile = nil
        return control(profile, action: client.discard)
      case .prepareRemoval(let profile):
        guard state.pendingProfile == nil,
          let model = state.profiles?.first(where: { $0.profile == profile }),
          model.state == .installed, model.deletionBlock == nil,
          let plan = client.removalPlan
        else { return .none }
        state.pendingProfile = profile
        return .run { send in
          do { await send(.removalPlanLoaded(try await plan(profile))) } catch {
            await send(.deletionRejected(profile, error.localizedDescription))
          }
        }
      case .removalPlanLoaded(let plan):
        guard state.pendingProfile == plan.profile else { return .none }
        state.pendingProfile = nil
        state.removalPlan = plan
        return .none
      case .cancelRemoval:
        state.removalPlan = nil
        state.feedback = nil
        state.feedbackProfile = nil
        return .none
      case .confirmRemoval:
        guard let plan = state.removalPlan, state.pendingProfile == nil,
          let delete = client.deleteConfirmed
        else { return .none }
        state.removalPlan = nil
        state.feedback = nil
        state.feedbackProfile = nil
        state.pendingProfile = plan.profile
        return .run { send in
          do {
            try await delete(plan.profile, plan.identity)
            await send(.deletionCompleted(plan.profile))
          } catch is ModelRemovalConflict {
            await send(.removalConflict(plan.profile))
          } catch {
            await send(.deletionRejected(plan.profile, error.localizedDescription))
          }
        }
      case .removalConflict(let profile):
        state.pendingProfile = nil
        state.feedbackProfile = nil
        state.feedback = "The model changed. Review the updated removal plan."
        return .send(.prepareRemoval(profile))
      case .delete(let profile):
        guard state.pendingProfile == nil,
          let model = state.profiles?.first(where: { $0.profile == profile }),
          model.state == .installed, model.deletionBlock == nil
        else { return .none }
        state.pendingProfile = profile
        state.feedback = nil
        state.feedbackProfile = nil
        return delete(profile)
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
      case .installationAccepted(let profile), .deletionCompleted(let profile):
        guard state.pendingProfile == profile else { return .none }
        guard state.isObserving else {
          state.pendingProfile = nil
          return .none
        }
        state.acceptedProfile = profile
        return observe()
      case .installationRejected(let profile, let message):
        guard state.pendingProfile == profile else { return .none }
        state.pendingProfile = nil
        state.feedbackProfile = profile
        state.feedback = message
        return .none
      case .deletionRejected(let profile, let message):
        guard state.pendingProfile == profile else { return .none }
        state.pendingProfile = nil
        state.feedbackProfile = nil
        state.feedback = message
        return .none
      }
    }
  }

  private func control(_ profile: String, action: @escaping ModelLibraryClient.Install) -> EffectOf<
    Self
  > {
    .run { send in
      do {
        try await action(profile)
        await send(.installationAccepted(profile))
      } catch {
        await send(.installationRejected(profile, error.localizedDescription))
      }
    }
  }

  private func delete(_ profile: String) -> EffectOf<Self> {
    .run { [client] send in
      do {
        try await client.delete(profile)
        await send(.deletionCompleted(profile))
      } catch {
        await send(.deletionRejected(profile, error.localizedDescription))
      }
    }
  }

  private func observeRuntime() -> EffectOf<Self> {
    guard let status = client.runtimeStatus else { return .none }
    let pollingClock = clock
    return .run { send in
      while !Task.isCancelled {
        do {
          let runtime = try await status()
          try Task.checkCancellation()
          await send(.runtimeLoaded(runtime))
        } catch is CancellationError {
          return
        } catch {
          await send(.runtimeFailed(error.localizedDescription))
        }
        try await pollingClock.sleep(for: .seconds(1))
      }
    }.cancellable(id: Cancellation.runtimeObservation, cancelInFlight: true)
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
        if let indexStatus = client.indexStatus {
          do {
            let index = try await indexStatus()
            try Task.checkCancellation()
            await send(.indexLoaded(index))
          } catch is CancellationError {
            return
          } catch {
            await send(.indexFailed(error.localizedDescription))
          }
        }
        if let storagePlan = client.storagePlan {
          do {
            let plan = try await storagePlan()
            try Task.checkCancellation()
            await send(.storageLoaded(plan))
          } catch is CancellationError {
            return
          } catch {
            await send(.storageFailed(error.localizedDescription))
          }
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
