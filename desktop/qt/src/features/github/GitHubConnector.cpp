#include "GitHubConnector.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QDateTime>
#include <QDesktopServices>
#include <QSet>

#include <algorithm>
#include <limits>

namespace ctxfab {

namespace {

using Features = openapi::GitHubFeatures;
using Configured = openapi::GitHubConfiguredRepository;

constexpr int DefaultMinimum = 15;
constexpr int DefaultMaximum = 1440;

template <typename T> bool hasRow(const QList<T> &items, int row) {
    return row >= 0 && row < items.size();
}

int flags(const Features &features) {
    return (features.isCurrentCode() ? GitHubConnector::BranchFiles : 0) |
           (features.isHistory() ? GitHubConnector::CommitHistory : 0) |
           (features.isActions() ? GitHubConnector::Actions : 0) |
           (features.isCodeGraph() ? GitHubConnector::CodeGraph : 0);
}

Features selection(int value) {
    Features features;
    features.setCurrentCode((value & GitHubConnector::BranchFiles) != 0);
    features.setHistory((value & GitHubConnector::CommitHistory) != 0);
    features.setActions((value & GitHubConnector::Actions) != 0);
    features.setCodeGraph((value & GitHubConnector::CodeGraph) != 0);
    return features;
}

} // namespace

GitHubConnector::GitHubConnector(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &GitHubConnector::backendChanged);
    collectionTimer_.setSingleShot(true);
    connect(&collectionTimer_, &QTimer::timeout, this, &GitHubConnector::pollCollection);
}

GitHubConnector::Credential GitHubConnector::credential() const {
    if (!state_) {
        return Unknown;
    }
    const auto value = state_->getCredential();
    if (value == QStringLiteral("connected")) {
        return Connected;
    }
    return value == QStringLiteral("reconnectRequired") ? ReconnectRequired : Disconnected;
}

QString GitHubConnector::login() const { return state_ ? state_->getLogin() : QString{}; }

QString GitHubConnector::feedback() const { return tr(qUtf8Printable(feedback_)); }

QString GitHubConnector::collectionFeedback() const {
    return tr(qUtf8Printable(collectionFeedback_));
}

QString GitHubConnector::draftDataFeedback() const {
    return tr(qUtf8Printable(draftDataFeedback_));
}

GitHubUsage GitHubConnector::usage() const {
    const auto traffic = state_ ? state_->getTraffic() : openapi::GitHubTraffic{};
    return GitHubUsage(traffic, storage_.value_or(openapi::GitHubStorageBudget{}));
}

void GitHubConnector::updateRepositoryList() {
    QList<GitHubRepositoryEntry> entries;
    if (state_) {
        for (const auto &item : state_->getRepositories()) {
            entries.append(GitHubRepositoryEntry(item));
        }
    }
    repositoryList_.replace(std::move(entries));
}

void GitHubConnector::retranslate() {
    updateRepositoryList();
    emit changed();
}

QUrl GitHubConnector::tokenUrl(bool actions, bool repositoryScoped) const {
    if (!state_) {
        return {};
    }
    auto templateUrl =
        actions ? state_->getActionsAuthorizationUrl() : state_->getAuthorizationUrl();
    if (repositoryScoped) {
        if (const auto repository = configuredDraft()) {
            const auto setup = repository->getTokenSetup();
            if (setup.isValid()) {
                templateUrl =
                    actions ? setup.getActionsAuthorizationUrl() : setup.getAuthorizationUrl();
            }
        }
    }
    const QUrl url(templateUrl, QUrl::StrictMode);
    if (url.scheme() != QStringLiteral("https") || url.host() != QStringLiteral("github.com") ||
        url.port(443) != 443 || !url.userInfo().isEmpty() ||
        url.path() != QStringLiteral("/settings/personal-access-tokens/new")) {
        return {};
    }
    return url;
}

void GitHubConnector::openTokenPage(bool actions, bool repositoryScoped) {
    const auto url = tokenUrl(actions, repositoryScoped);
    if (!url.isEmpty()) {
        QDesktopServices::openUrl(url);
    }
}

void GitHubConnector::backendChanged() {
    stopCollectionPoll();
    request_.cancel();
    state_.reset();
    draft_.reset();
    storage_.reset();
    clearDraftData();
    busy_ = false;
    review_ = false;
    collectionFeedback_.clear();
    clearBranches();
    clearDiscovery();
    updateRepositoryList();
    if (collectionVisible_) {
        reload();
    } else {
        emit changed();
    }
}

template <typename Payload, typename Handler>
GitHubConnector::Api *GitHubConnector::begin(void (Api::*response)(Worker *, Payload),
                                             ErrorSignal error, Handler handler,
                                             ConflictHandling conflict) {
    auto *api = beginCommand(error, conflict);
    if (api) {
        connect(api, response, this, [this, handler](const Worker *worker, const Payload &payload) {
            if (worker->getHttpResponseCode() == 200 && payload.isValid()) {
                handler(payload);
            } else {
                finish(QT_TR_NOOP("Invalid response from the backend"));
            }
        });
    }
    return api;
}

GitHubConnector::Api *GitHubConnector::beginCommand(ErrorSignal error, ConflictHandling conflict) {
    auto *api = startCommand();
    if (api) {
        connect(api, error, this,
                [this, conflict](const Worker *worker) { rejected(*worker, conflict); });
    }
    return api;
}

GitHubConnector::Api *GitHubConnector::startCommand(std::optional<int> timeoutMs) {
    if (busy_) {
        return nullptr;
    }
    stopCollectionPoll();
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Backend session unavailable"));
        return nullptr;
    }
    auto *api = request_.begin(*session, timeoutMs);
    busy_ = true;
    feedback_.clear();
    emit changed();
    return api;
}

GitHubConnector::Api *GitHubConnector::beginState(StateSignal response, ErrorSignal error,
                                                  DraftResult draft, ConflictHandling conflict) {
    return begin(
        response, error,
        [this, draft](const openapi::GitHubState &state) { acceptState(state, draft); }, conflict);
}

void GitHubConnector::acceptState(const openapi::GitHubState &state, DraftResult draft,
                                  const QString &feedback) {
    if (!validState(state)) {
        finish(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    updateState(state, draft);
    finish(feedback);
}

bool GitHubConnector::validState(const openapi::GitHubState &state) const {
    return state.isValid() && state.getRevision() >= 0 &&
           state.getRevision() <= std::numeric_limits<quint32>::max() &&
           (!state_ || state.getRevision() >= state_->getRevision());
}

void GitHubConnector::updateState(const openapi::GitHubState &state, DraftResult draft) {
    state_ = state;
    collectionFeedback_.clear();
    if (draft == DraftResult::Close) {
        draft_.reset();
        clearDraftData();
    } else if (draft == DraftResult::Verify && draft_) {
        draft_->setRevision(state.getRevision());
    }
    review_ = draft_ && draft_->getRevision() != state_->getRevision();
    if (credential() != Connected) {
        clearDiscovery();
    } else {
        buildCandidateRows();
    }
    updateRepositoryList();
}

void GitHubConnector::rejected(const Worker &worker, ConflictHandling conflict) {
    if (conflict == ConflictHandling::ReviewDraft && worker.getHttpResponseCode() == 409 &&
        draft_) {
        review_ = true;
        finish(QT_TR_NOOP("GitHub settings changed. Review your edits before saving."));
        return;
    }
    finish(LocalApiRequest::rejection(worker, QT_TR_NOOP("Backend request failed")));
}

void GitHubConnector::finish(const QString &feedback) {
    request_.cancel();
    busy_ = false;
    feedback_ = feedback;
    scheduleCollectionPoll();
    emit changed();
}

void GitHubConnector::reload() {
    if (auto *api =
            beginState(&Api::readGitHubStateSignalFull, &Api::readGitHubStateSignalErrorFull)) {
        api->readGitHubState();
        loadDraftData();
    }
}

void GitHubConnector::setCollectionVisible(bool visible) {
    if (collectionVisible_ == visible) {
        return;
    }
    collectionVisible_ = visible;
    if (visible) {
        reload();
    } else {
        stopCollectionPoll();
    }
}

void GitHubConnector::scheduleCollectionPoll() {
    if (!collectionVisible_ || busy_ || !state_ || !collectionFeedback_.isEmpty()) {
        return;
    }
    const auto now = QDateTime::currentMSecsSinceEpoch();
    int delay = -1;
    const auto repositories = state_->getRepositories();
    for (const auto &repository : repositories) {
        const int candidate = GitHubCollection(repository.getCodeCollection()).pollDelay(now);
        if (candidate >= 0 && (delay < 0 || candidate < delay)) {
            delay = candidate;
        }
    }
    if (delay >= 0) {
        collectionTimer_.start(delay);
    }
}

void GitHubConnector::stopCollectionPoll() {
    collectionTimer_.stop();
    if (polling_) {
        request_.cancel();
        polling_ = false;
    }
}

void GitHubConnector::pollCollection() {
    const auto session = backend_.session();
    if (!session) {
        finishCollectionPoll(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    auto *api = request_.begin(*session);
    polling_ = true;
    connect(api, &Api::readGitHubStateSignalFull, this,
            [this](const Worker *worker, const openapi::GitHubState &state) {
                if (worker->getHttpResponseCode() == 200 && validState(state)) {
                    updateState(state, DraftResult::Preserve);
                    finishCollectionPoll();
                } else {
                    finishCollectionPoll(QT_TR_NOOP("Invalid response from the backend"));
                }
            });
    connect(api, &Api::readGitHubStateSignalErrorFull, this, [this](const Worker *worker) {
        finishCollectionPoll(
            LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    api->readGitHubState();
}

void GitHubConnector::finishCollectionPoll(const QString &feedback) {
    stopCollectionPoll();
    collectionFeedback_ = feedback;
    scheduleCollectionPoll();
    emit changed();
}

GitHubCollection GitHubConnector::draftCollection() const {
    const auto repo = configuredDraft();
    if (!repo) {
        return {};
    }
    const auto features = repo->getFeatures();
    return GitHubCollection(repo->getCodeCollection(),
                            features.isCodeGraph() && !features.isCurrentCode());
}

bool GitHubConnector::canRefreshBranch() const {
    if (credential() != Connected || review_ || !editing()) {
        return false;
    }
    const auto repo = configured(draft_->getExpectedId());
    if (!repo) {
        return false;
    }
    if (repo->getCodeCollection().isCanRefresh()) {
        return true;
    }
    const auto features = repo->getFeatures();
    return draftDataPaused() && (features.isCurrentCode() || features.isCodeGraph()) &&
           repo->getContents().getValue() == openapi::GitHubAccess::eGitHubAccess::AVAILABLE;
}

void GitHubConnector::refreshBranch() {
    if (!canRefreshBranch()) {
        return;
    }
    const auto id = draft_->getExpectedId();
    if (auto *api = beginCommand(&Api::refreshGitHubCurrentCodeSignalErrorFull,
                                 ConflictHandling::ReviewDraft)) {
        draftDataRequest_.cancel();
        connect(
            api, &Api::refreshGitHubCurrentCodeSignalFull, this,
            [this](const Worker *worker, const openapi::RefreshGitHubCurrentCode_200_response &) {
                const openapi::GitHubState state(QString::fromUtf8(worker->response));
                if (worker->getHttpResponseCode() != 202 || !validState(state)) {
                    finish(QT_TR_NOOP("Invalid response from the backend"));
                    return;
                }
                draftDataPaused_ = false;
                draftDataFeedback_.clear();
                updateState(state, DraftResult::Preserve);
                finish();
            });
        api->refreshGitHubCurrentCode(id, true);
    }
}

std::optional<Configured> GitHubConnector::graphRepository() const {
    const auto repo = configuredDraft();
    return repo && repo->getFeatures().isCodeGraph() ? repo : std::nullopt;
}

bool GitHubConnector::hasCommitHistory() const {
    const auto repo = configuredDraft();
    return repo && repo->getFeatures().isHistory();
}

bool GitHubConnector::canRefreshHistory() const {
    const auto repo = configuredDraft();
    return repo && repo->getFeatures().isHistory() && credential() == Connected &&
           repo->isVerified() &&
           repo->getContents().getValue() == openapi::GitHubAccess::eGitHubAccess::AVAILABLE;
}

QList<GitHubCollectionTraceLink> GitHubConnector::historyTraces() const {
    const auto repo = configuredDraft();
    return repo ? GitHubCollectionTraceLink::links(repo->getHistoryActivity())
                : QList<GitHubCollectionTraceLink>{};
}

QList<GitHubCollectionTraceLink> GitHubConnector::actionsTraces() const {
    const auto repo = configuredDraft();
    return repo ? GitHubCollectionTraceLink::links(repo->getActionsActivity())
                : QList<GitHubCollectionTraceLink>{};
}

void GitHubConnector::refreshHistory() {
    if (!canRefreshHistory()) {
        return;
    }
    const auto repo = *configuredDraft();
    if (auto *api = startCommand(90000)) {
        connect(api, &Api::refreshGitHubHistorySignalFull, this,
                [this, api, branch = repo.getBranch()](const Worker *worker,
                                                       const openapi::GitHubHistory &history) {
                    const bool published = worker->getHttpResponseCode() == 200 &&
                                           history.isValid() && history.getBranch() == branch;
                    loadDraftData();
                    readCollectionState(
                        *api, published ? QT_TR_NOOP("Code history refreshed.")
                                        : QT_TR_NOOP("Invalid response from the backend"));
                });
        connect(api, &Api::refreshGitHubHistorySignalErrorFull, this,
                [this, api](const Worker *worker) {
                    loadDraftData();
                    readCollectionState(*api, LocalApiRequest::rejection(
                                                  *worker, QT_TR_NOOP("Backend request failed")));
                });
        api->refreshGitHubHistory(repo.getRepository().getId());
    }
}

bool GitHubConnector::canBrowseGraph() const { return graphRepository().has_value(); }

void GitHubConnector::browseGraph() {
    if (const auto repo = graphRepository(); repo && !busy_) {
        const auto identity = repo->getRepository();
        emit graphRequested(identity.getId(), identity.getFullName(), repo->getBranch());
    }
}

void GitHubConnector::refreshGraphSource(const QString &branch) {
    if (const auto repo = graphRepository(); repo && repo->getBranch() == branch) {
        refreshBranch();
    }
}

bool GitHubConnector::updateStorage(const openapi::GitHubStorageBudget &budget) {
    const auto revision = budget.getRevision();
    const auto limit = budget.getLimitGb();
    if (revision < 0 || revision > std::numeric_limits<quint32>::max() || limit < 1 ||
        limit > 1000 || budget.getBytes() < 0) {
        return false;
    }
    storage_ = budget;
    return true;
}

void GitHubConnector::acceptStorage(const openapi::GitHubStorageBudget &budget) {
    finish(updateStorage(budget) ? QString{} : QT_TR_NOOP("Invalid response from the backend"));
}

void GitHubConnector::loadStorage() {
    if (auto *api = begin(
            &Api::readGitHubStorageSignalFull, &Api::readGitHubStorageSignalErrorFull,
            [this](const auto &budget) { acceptStorage(budget); },
            ConflictHandling::ReportProblem)) {
        api->readGitHubStorage();
    }
}

void GitHubConnector::setStorageLimit(int limit) {
    if (!storage_ || limit < 1 || limit > 1000 || limit == storage_->getLimitGb()) {
        return;
    }
    if (auto *api = begin(
            &Api::setGitHubStorageSignalFull, &Api::setGitHubStorageSignalErrorFull,
            [this](const auto &budget) { acceptStorage(budget); },
            ConflictHandling::ReportProblem)) {
        openapi::SetGitHubStorage_request request;
        request.setRevision(storage_->getRevision());
        request.setLimitGb(limit);
        api->setGitHubStorage(request);
    }
}

void GitHubConnector::setRequestLimit(int limit) {
    const auto current = usage();
    if (!current.hasTraffic() || limit < 1 || limit > current.maximumRate() ||
        limit == current.requestedRate()) {
        return;
    }
    if (auto *api = beginState(&Api::updateGitHubRateLimitSignalFull,
                               &Api::updateGitHubRateLimitSignalErrorFull, DraftResult::Preserve,
                               ConflictHandling::ReportProblem)) {
        openapi::GitHubRateLimit request;
        request.setRevision(state_->getRevision());
        request.setRequestsPerMinute(limit);
        api->updateGitHubRateLimit(request);
    }
}

void GitHubConnector::clearDraftData() {
    draftDataRequest_.cancel();
    draftDataPaused_.reset();
    draftDataFeedback_.clear();
}

void GitHubConnector::finishDraftData(const QString &feedback) {
    draftDataRequest_.cancel();
    draftDataFeedback_ = feedback;
    emit changed();
}

void GitHubConnector::loadDraftData() {
    if (!editing()) {
        return;
    }
    const auto session = backend_.session();
    if (!session) {
        finishDraftData(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    auto *api = draftDataRequest_.begin(*session);
    connect(api, &Api::readGitHubRepositoryDataStateSignalFull, this,
            [this](const Worker *worker,
                   const openapi::ReadGitHubRepositoryDataState_200_response &data) {
                if (worker->getHttpResponseCode() != 200 || !data.isValid()) {
                    finishDraftData(QT_TR_NOOP("Invalid response from the backend"));
                    return;
                }
                draftDataPaused_ = data.isPaused();
                finishDraftData();
            });
    connect(api, &Api::readGitHubRepositoryDataStateSignalErrorFull, this,
            [this](const Worker *worker) {
                finishDraftData(
                    LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
            });
    api->readGitHubRepositoryDataState(draft_->getExpectedId());
}

void GitHubConnector::clearPurgedCollection(qint64 id) {
    auto repositories = state_->getRepositories();
    const auto repository = std::ranges::find(repositories, id, [](const Configured &configured) {
        return configured.getRepository().getId();
    });
    if (repository != repositories.end()) {
        repository->setCodeCollection({});
        repository->setHistoryActivity({});
        repository->setActionsActivity({});
        state_->setRepositories(repositories);
    }
}

void GitHubConnector::acceptPurge(Api &api, qint64 id, const Worker &worker,
                                  const openapi::GitHubStorageBudget &budget) {
    if (worker.getHttpResponseCode() != 200 || !budget.isValid() || !updateStorage(budget)) {
        finish(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    draftDataPaused_ = true;
    draftDataFeedback_.clear();
    clearPurgedCollection(id);
    emit repositoryPurged(id);
    readCollectionState(api);
}

void GitHubConnector::readCollectionState(Api &api, const QString &feedback) {
    connect(&api, &Api::readGitHubStateSignalFull, this,
            [this, feedback](const Worker *reply, const openapi::GitHubState &snapshot) {
                if (reply->getHttpResponseCode() == 200) {
                    acceptState(snapshot, DraftResult::Preserve, feedback);
                } else {
                    finish(QT_TR_NOOP("Invalid response from the backend"));
                }
            });
    connect(&api, &Api::readGitHubStateSignalErrorFull, this,
            [this](const Worker *reply) { rejected(*reply, ConflictHandling::ReportProblem); });
    api.readGitHubState();
}

void GitHubConnector::purgeRepositoryData() {
    if (!editing() || review_) {
        return;
    }
    const auto id = draft_->getExpectedId();
    if (auto *api = beginCommand(&Api::purgeGitHubRepositoryDataSignalErrorFull,
                                 ConflictHandling::ReportProblem)) {
        draftDataRequest_.cancel();
        connect(api, &Api::purgeGitHubRepositoryDataSignalFull, this,
                [this, api, id](const Worker *worker, const openapi::GitHubStorageBudget &budget) {
                    acceptPurge(*api, id, *worker, budget);
                });
        api->purgeGitHubRepositoryData(id);
    }
}

void GitHubConnector::authorize(const QString &token) {
    if (!state_ || busy_ || token.isEmpty()) {
        return;
    }
    clearDiscovery();
    if (auto *api =
            beginState(&Api::authorizeGitHubSignalFull, &Api::authorizeGitHubSignalErrorFull)) {
        openapi::GitHubAuthorization request;
        request.setRevision(state_->getRevision());
        request.setToken(token);
        api->authorizeGitHub(request);
    }
}

void GitHubConnector::disconnectAccount() {
    if (credential() == Disconnected || !state_) {
        return;
    }
    if (auto *api = beginState(&Api::disconnectGitHubSignalFull,
                               &Api::disconnectGitHubSignalErrorFull, DraftResult::Close)) {
        api->disconnectGitHub(state_->getRevision());
    }
}

void GitHubConnector::clearDiscovery() {
    discovered_.clear();
    candidateList_.replace({});
    candidatePage_ = 0;
}

void GitHubConnector::discover() {
    if (credential() == Connected && !busy_) {
        clearDiscovery();
        loadCandidates(1);
    }
}

void GitHubConnector::loadMoreCandidates() {
    if (moreCandidates()) {
        loadCandidates(candidatePage_);
    }
}

void GitHubConnector::buildCandidateRows() {
    QSet<qint64> configuredIds;
    if (state_) {
        for (const auto &item : state_->getRepositories()) {
            configuredIds.insert(item.getRepository().getId());
        }
    }
    QList<GitHubRepositoryEntry> entries;
    for (const auto &repository : discovered_) {
        if (!configuredIds.contains(repository.getId()) &&
            repository.getFullName().contains(filter_, Qt::CaseInsensitive)) {
            entries.append(GitHubRepositoryEntry(repository));
        }
    }
    candidateList_.replace(std::move(entries));
}

void GitHubConnector::filterCandidates(const QString &filter) {
    filter_ = filter.trimmed();
    buildCandidateRows();
    emit changed();
}

void GitHubConnector::loadCandidates(int page) {
    auto *api = begin(
        &Api::discoverGitHubRepositoriesSignalFull, &Api::discoverGitHubRepositoriesSignalErrorFull,
        [this, page](const openapi::GitHubDiscovery &result) {
            for (const auto &repository : result.getRepositories()) {
                const auto duplicate = std::ranges::any_of(discovered_, [&](const auto &item) {
                    return item.getId() == repository.getId();
                });
                if (!duplicate) {
                    discovered_.append(repository);
                }
            }
            candidatePage_ = result.getNextPage() > page ? result.getNextPage() : 0;
            const auto traffic = result.getTraffic();
            if (traffic.isValid()) {
                state_->setTraffic(traffic);
            }
            buildCandidateRows();
            finish();
        });
    if (api) {
        api->discoverGitHubRepositories(page);
    }
}

void GitHubConnector::createDraft(const QString &repository, const QString &branch, qint64 id) {
    const auto defaults = state_->getDefaults();
    draft_.emplace();
    draft_->setRepository(repository);
    draft_->setBranch(branch);
    draft_->setRevision(state_->getRevision());
    draft_->setIntervalMinutes(defaults.getIntervalMinutes());
    draft_->setFeatures(defaults.getFeatures());
    if (id > 0) {
        draft_->setExpectedId(id);
    }
    clearBranches();
    clearDraftData();
    review_ = false;
    feedback_.clear();
}

void GitHubConnector::addRepository(const QString &repository) {
    if (credential() == Connected && !busy_) {
        createDraft(repository.trimmed(), {}, 0);
        emit changed();
    }
}

void GitHubConnector::chooseCandidate(int row) {
    const auto candidate = candidateList_.entryAt(row);
    if (credential() == Connected && !busy_ && candidate.id() > 0) {
        createDraft(candidate.name(), candidate.branch(), 0);
        emit changed();
    }
}

void GitHubConnector::editRepository(int row) {
    if (!state_ || busy_) {
        return;
    }
    const auto items = state_->getRepositories();
    if (hasRow(items, row)) {
        const auto &item = items.at(row);
        createDraft(item.getRepository().getFullName(), item.getBranch(),
                    item.getRepository().getId());
        draft_->setIntervalMinutes(item.getIntervalMinutes());
        draft_->setFeatures(item.getFeatures());
        emit changed();
        loadDraftData();
    }
}

std::optional<Configured> GitHubConnector::configured(qint64 id) const {
    if (state_) {
        for (const auto &item : state_->getRepositories()) {
            if (item.getRepository().getId() == id) {
                return item;
            }
        }
    }
    return std::nullopt;
}

std::optional<Configured> GitHubConnector::configuredDraft() const {
    return editing() ? configured(draft_->getExpectedId()) : std::nullopt;
}

bool GitHubConnector::draftCanReview() const {
    return draft_ && state_ && draft_->getRevision() != state_->getRevision() &&
           (!editing() || configured(draft_->getExpectedId()).has_value());
}

QString GitHubConnector::draftRepository() const {
    return draft_ ? draft_->getRepository() : QString{};
}

QString GitHubConnector::draftBranch() const { return draft_ ? draft_->getBranch() : QString{}; }

int GitHubConnector::draftInterval() const {
    return draft_ ? draft_->getIntervalMinutes() : DefaultMinimum;
}

int GitHubConnector::draftFeatures() const { return draft_ ? flags(draft_->getFeatures()) : 0; }

bool GitHubConnector::draftVerified() const {
    const auto item = configuredDraft();
    return item && item->isVerified();
}

int GitHubConnector::minimumInterval() const {
    const auto bound = state_ ? state_->getDefaults().getRefreshInterval().getMinimumMinutes() : 0;
    return bound > 0 ? bound : DefaultMinimum;
}

int GitHubConnector::maximumInterval() const {
    const auto bound = state_ ? state_->getDefaults().getRefreshInterval().getMaximumMinutes() : 0;
    return bound > 0 ? bound : DefaultMaximum;
}

void GitHubConnector::loadMoreBranches() {
    if (editing() && credential() == Connected && !review_ && !busy_) {
        loadBranches(branchPage_ > 0 ? branchPage_ : 1);
    }
}

QStringList GitHubConnector::matchingBranches() const {
    return branches_.filter(branchFilter_, Qt::CaseInsensitive);
}

void GitHubConnector::filterBranches(const QString &filter) {
    branchFilter_ = filter.trimmed();
    emit changed();
}

void GitHubConnector::clearBranches() {
    branches_.clear();
    branchFilter_.clear();
    branchPage_ = 0;
    branchesLoaded_ = false;
}

void GitHubConnector::loadBranches(int page) {
    auto *api = begin(&Api::listGitHubRepositoryBranchesSignalFull,
                      &Api::listGitHubRepositoryBranchesSignalErrorFull,
                      [this, page](const openapi::GitHubBranchPage &result) {
                          if (page == 1) {
                              branches_.clear();
                          }
                          for (const auto &branch : result.getBranches()) {
                              if (!branches_.contains(branch)) {
                                  branches_.append(branch);
                              }
                          }
                          branchPage_ = result.getNextPage() > page ? result.getNextPage() : 0;
                          branchesLoaded_ = true;
                          finish();
                      });
    if (api) {
        api->listGitHubRepositoryBranches(draft_->getExpectedId(), state_->getRevision(), page);
    }
}

void GitHubConnector::reviewDraft(const QString &repository, const QString &branch, int interval,
                                  int features) {
    if (!review_ || busy_ || !draftCanReview()) {
        return;
    }
    const auto latest = editing() ? configured(draft_->getExpectedId()) : std::nullopt;
    if (latest) {
        const auto editedFlags = features ^ draftFeatures();
        draft_->setRepository(latest->getRepository().getFullName());
        draft_->setBranch(branch == draftBranch() ? latest->getBranch() : branch);
        draft_->setIntervalMinutes(interval == draftInterval() ? latest->getIntervalMinutes()
                                                               : interval);
        draft_->setFeatures(
            selection((flags(latest->getFeatures()) & ~editedFlags) | (features & editedFlags)));
    } else {
        draft_->setRepository(repository);
        draft_->setBranch(branch);
        draft_->setIntervalMinutes(interval);
        draft_->setFeatures(selection(features));
    }
    draft_->setRevision(state_->getRevision());
    review_ = false;
    clearBranches();
    feedback_.clear();
    emit changed();
}

void GitHubConnector::closeDraft() {
    if (draft_ && !busy_) {
        draft_.reset();
        clearDraftData();
        review_ = false;
        emit changed();
    }
}

void GitHubConnector::saveRepository(const QString &repository, const QString &branch, int interval,
                                     int features) {
    if (!draft_ || review_ || credential() != Connected || repository.trimmed().isEmpty() ||
        interval < minimumInterval() || interval > maximumInterval()) {
        return;
    }
    if (auto *api = beginState(&Api::saveGitHubRepositorySignalFull,
                               &Api::saveGitHubRepositorySignalErrorFull, DraftResult::Close)) {
        openapi::GitHubSelection request;
        request.setRevision(draft_->getRevision());
        request.setRepository(repository);
        request.setIntervalMinutes(interval);
        request.setFeatures(selection(features));
        if (!branch.isEmpty()) {
            request.setBranch(branch);
        }
        if (editing()) {
            request.setExpectedId(draft_->getExpectedId());
            request.setRepository(draft_->getRepository());
        }
        api->saveGitHubRepository(request);
    }
}

void GitHubConnector::verifyAccess() {
    if (configuredDraft() && credential() == Connected) {
        if (auto *api = beginState(&Api::checkGitHubRepositoryAccessSignalFull,
                                   &Api::checkGitHubRepositoryAccessSignalErrorFull,
                                   review_ ? DraftResult::Preserve : DraftResult::Verify)) {
            api->checkGitHubRepositoryAccess(draft_->getExpectedId(), state_->getRevision());
        }
    }
}

void GitHubConnector::removeRepository() {
    if (editing() && !review_) {
        if (auto *api =
                beginState(&Api::removeGitHubRepositorySignalFull,
                           &Api::removeGitHubRepositorySignalErrorFull, DraftResult::Close)) {
            api->removeGitHubRepository(draft_->getExpectedId(), draft_->getRevision());
        }
    }
}

} // namespace ctxfab
