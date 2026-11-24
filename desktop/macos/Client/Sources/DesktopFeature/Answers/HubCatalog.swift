import ComposableArchitecture
import Foundation
import LocalAPI

public struct HubCatalogClient: Sendable {
  public typealias Repository = Components.Schemas.HubModelRepository
  public typealias Variant = Components.Schemas.HubModelVariant

  public typealias Search = @Sendable (String, HubModelKind) async throws -> [Repository]
  public typealias ReadVariants = @Sendable (Repository, HubModelKind) async throws -> [Variant]
  public typealias Install = @Sendable (Repository, Variant, HubModelKind) async throws -> String

  let search: Search
  let variants: ReadVariants
  let install: Install

  public init(
    search: @escaping Search = { _, _ in
      throw BackendConnectionError.helperExited
    },
    variants: @escaping ReadVariants = { _, _ in
      throw BackendConnectionError.helperExited
    },
    install: @escaping Install = { _, _, _ in
      throw BackendConnectionError.helperExited
    }
  ) {
    self.search = search
    self.variants = variants
    self.install = install
  }
}

@Reducer
public struct HubCatalog {
  public typealias Repository = HubCatalogClient.Repository
  public typealias Variant = HubCatalogClient.Variant

  public enum Operation: Equatable {
    case searching
    case loadingVariants
    case addingDownload
  }

  @ObservableState
  public struct State: Equatable {
    public internal(set) var kind: HubModelKind = .generation
    public internal(set) var query = ""
    public internal(set) var repositories: [Repository]?
    public internal(set) var repository: Repository?
    public internal(set) var variants: [Variant]?
    public internal(set) var selectedVariant: Variant?
    public internal(set) var operation: Operation?
    public internal(set) var feedback: String?
  }

  public enum Action {
    case selectKind(HubModelKind)
    case search(String)
    case repositoriesLoaded([Repository])
    case openRepository(Repository)
    case variantsLoaded([Variant])
    case selectVariant(Variant)
    case download
    case downloadAccepted(String)
    case failed(String)
    case back
    case close
  }

  private let client: HubCatalogClient
  private enum Cancellation { case request }

  init(client: HubCatalogClient) { self.client = client }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .selectKind(let kind):
        guard state.operation != .addingDownload, state.kind != kind else { return .none }
        state = State()
        state.kind = kind
        return .cancel(id: Cancellation.request)
      case .search(let query):
        guard state.operation == nil, state.repository == nil else { return .none }
        state.query = query
        state.repositories = nil
        state.feedback = nil
        state.operation = .searching
        return search(state.query, kind: state.kind)
      case .repositoriesLoaded(let repositories):
        state.operation = nil
        state.repositories = repositories
        return .none
      case .openRepository(let repository):
        guard state.operation == nil, state.repositories?.contains(repository) == true else {
          return .none
        }
        state.repository = repository
        state.feedback = nil
        state.operation = .loadingVariants
        return load(repository, kind: state.kind)
      case .variantsLoaded(let variants):
        state.operation = nil
        state.variants = variants
        return .none
      case .selectVariant(let variant):
        guard state.operation == nil, state.variants?.contains(variant) == true,
          state.kind != .embedding || variant.sha256?.isEmpty == false
        else {
          return .none
        }
        state.selectedVariant = variant
        return .none
      case .download:
        guard state.operation == nil, let repository = state.repository,
          let file = state.selectedVariant,
          state.kind != .embedding || file.sha256?.isEmpty == false
        else { return .none }
        state.operation = .addingDownload
        state.feedback = nil
        return install(repository, file, kind: state.kind)
      case .failed(let feedback):
        state.operation = nil
        state.feedback = feedback
        return .none
      case .back:
        guard state.operation != .addingDownload else { return .none }
        state.repository = nil
        state.variants = nil
        state.selectedVariant = nil
        state.operation = nil
        state.feedback = nil
        return .cancel(id: Cancellation.request)
      case .close, .downloadAccepted:
        state = State()
        return .cancel(id: Cancellation.request)
      }
    }
  }

  private func search(_ query: String, kind: HubModelKind) -> EffectOf<Self> {
    .run { [client] send in
      do {
        await send(.repositoriesLoaded(try await client.search(query, kind)))
      } catch {
        if !Task.isCancelled { await send(.failed(error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.request, cancelInFlight: true)
  }

  private func load(_ repository: Repository, kind: HubModelKind) -> EffectOf<Self> {
    .run { [client] send in
      do {
        await send(.variantsLoaded(try await client.variants(repository, kind)))
      } catch {
        if !Task.isCancelled { await send(.failed(error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.request, cancelInFlight: true)
  }

  private func install(
    _ repository: Repository,
    _ file: Variant,
    kind: HubModelKind
  ) -> EffectOf<Self> {
    .run { [client] send in
      do {
        await send(.downloadAccepted(try await client.install(repository, file, kind)))
      } catch {
        if !Task.isCancelled { await send(.failed(error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.request, cancelInFlight: true)
  }
}
