#include "ModelLibrary.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QDir>
#include <QJsonDocument>
#include <QSet>

#include <algorithm>
#include <utility>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
using Action = LibraryModel::Action;
constexpr int RefreshIntervalMs = 1000;
constexpr int ModelCommandTimeoutMs = 10000;

bool validCatalog(const QList<openapi::ModelDownload> &models) {
    QSet<QString> profiles;
    for (const auto &model : models) {
        if (!LibraryModel::accepts(model) || profiles.contains(model.getProfile())) {
            return false;
        }
        profiles.insert(model.getProfile());
    }
    return true;
}

} // namespace

ModelLibrary::ModelLibrary(BackendSupervisor &backend)
    : backend_(backend), commandRequest_(ModelCommandTimeoutMs) {
    poll_.setInterval(RefreshIntervalMs);
    connect(&poll_, &QTimer::timeout, this, &ModelLibrary::refresh);
    connect(&backend_, &BackendSupervisor::changed, this, &ModelLibrary::resetSession);
}

bool ModelLibrary::ready() const { return backend_.session().has_value(); }

bool ModelLibrary::transferActive() const {
    for (int row = 0; row < downloads_.count(); ++row) {
        if (downloads_.entryAt(row).transferring()) {
            return true;
        }
    }
    return false;
}

QString ModelLibrary::catalogFeedback() const {
    return catalogFailed_ ? tr("Could not load the model library.") : QString{};
}

QString ModelLibrary::feedback() const {
    return staleRemoval_ ? tr("The model changed. Review the updated removal plan.")
                         : commandFeedback_;
}

QString ModelLibrary::removalMessage() const {
    if (!removal_.isValid()) {
        return {};
    }
    const auto prefix = staleRemoval_ ? feedback() + QStringLiteral("\n\n") : QString{};
    return tr("%1Delete the local weights for %2 (%3 of logical data)? You can download them "
              "again.")
        .arg(prefix, removal_.getProfile(), formatArtifactSize(removal_.getBytes()));
}

QString ModelLibrary::storageSummary() const {
    if (!storage_.is_capacity_Set()) {
        return {};
    }
    const auto capacity = storage_.getCapacity();
    return tr("Inference storage: %1 used of %2")
        .arg(formatArtifactSize(capacity.getUsedBytes()),
             formatArtifactSize(capacity.getLimitBytes()));
}

QString ModelLibrary::storageDetails() const {
    if (!storage_.is_capacity_Set()) {
        return {};
    }
    const auto capacity = storage_.getCapacity();
    return tr("Reserved: %1 · Available: %2")
        .arg(formatArtifactSize(capacity.getReservedBytes()),
             formatArtifactSize(capacity.getAvailableBytes()));
}

QString ModelLibrary::storageFeedback() const {
    return storageFailed_ ? tr("Could not estimate inference storage.") : QString{};
}

QString ModelLibrary::runtimeImportEstimate() const {
    return runtimeEstimate(storage_.getRuntime().getLocalImport(),
                           QT_TR_NOOP("ZIP import peak: %1"));
}

QString ModelLibrary::runtimeDownloadEstimate() const {
    return runtimeEstimate(storage_.getRuntime().getHostedDownload(),
                           QT_TR_NOOP("Download peak: %1"));
}

QString ModelLibrary::runtimeEstimate(const openapi::InstallStoragePlan &plan,
                                      const char *label) const {
    if (!plan.isValid()) {
        return {};
    }
    const auto estimate = tr(label).arg(formatArtifactSize(plan.getPeakBytes()));
    return plan.isFits() ? estimate : tr("%1 · Insufficient available space").arg(estimate);
}

void ModelLibrary::setQuery(const QString &query) {
    if (query_ != query) {
        query_ = query;
        rebuildEntries();
        emit queryChanged();
    }
}

void ModelLibrary::retranslate() {
    downloads_.retranslate();
    answerModels_.retranslate();
    embeddingModels_.retranslate();
    otherModels_.retranslate();
    setupAnswers_.retranslate();
    setupEmbeddings_.retranslate();
    emit changed();
}

void ModelLibrary::open() {
    poll_.start();
    refresh();
    emit changed();
}

void ModelLibrary::close() {
    poll_.stop();
    cancelReads();
    cancelRemoval();
    if (command_ == CommandState::PreparingRemoval || command_ == CommandState::AwaitingCatalog) {
        endCommand();
    }
    catalogReady_ = false;
    emit changed();
}

void ModelLibrary::resetSession() {
    cancelReads();
    endCommand();
    removal_ = {};
    staleRemoval_ = false;
    feedbackProfile_.clear();
    commandFeedback_.clear();
    catalog_.clear();
    storage_ = {};
    catalogReady_ = false;
    catalogFailed_ = false;
    storageFailed_ = false;
    rebuildEntries();
    emit changed();
    refresh();
}

void ModelLibrary::cancelReads() {
    catalogRequest_.cancel();
    storageRequest_.cancel();
    catalogBusy_ = false;
    storageBusy_ = false;
}

void ModelLibrary::refresh() {
    if (!poll_.isActive() || !ready()) {
        return;
    }
    if (!catalogBusy_) {
        loadCatalog();
    }
    if (!storageBusy_) {
        loadStorage();
    }
}

QUrl ModelLibrary::modelFolder() const {
    if (!ready()) {
        return {};
    }
    const QDir profile(backend_.profileDirectory());
    return profile.mkpath(QStringLiteral("inference"))
               ? QUrl::fromLocalFile(profile.filePath(QStringLiteral("inference")))
               : QUrl{};
}

void ModelLibrary::loadCatalog() {
    const auto session = backend_.session();
    if (!session) {
        return;
    }
    auto *api = catalogRequest_.begin(*session);
    connect(api, &Api::readModelDownloadsSignalFull, this,
            [this](const Worker *worker, const QList<openapi::ModelDownload> &models) {
                receiveCatalog(*worker, models);
            });
    connect(api, &Api::readModelDownloadsSignalErrorFull, this,
            [this](const Worker *worker) { receiveCatalog(*worker, {}); });
    catalogBusy_ = true;
    api->readModelDownloads();
}

void ModelLibrary::receiveCatalog(const Worker &worker,
                                  const QList<openapi::ModelDownload> &models) {
    catalogBusy_ = false;
    catalogRequest_.cancel();
    if (command_ == CommandState::AwaitingCatalog) {
        endCommand();
    }
    catalogFailed_ = worker.getHttpResponseCode() != 200 ||
                     !QJsonDocument::fromJson(worker.response).isArray() || !validCatalog(models);
    if (!catalogFailed_) {
        const auto installed = installedProfiles();
        catalogReady_ = true;
        catalog_ = models;
        rebuildEntries();
        if (installed != installedProfiles()) {
            emit installedModelsChanged();
        }
    }
    emit changed();
}

void ModelLibrary::loadStorage() {
    const auto session = backend_.session();
    if (!session) {
        return;
    }
    auto *api = storageRequest_.begin(*session);
    connect(api, &Api::getInferenceStorageSignalFull, this,
            [this](const Worker *worker, const openapi::InferenceStoragePlan &plan) {
                receiveStorage(*worker, plan);
            });
    connect(api, &Api::getInferenceStorageSignalErrorFull, this,
            [this](const Worker *worker) { receiveStorage(*worker, {}); });
    storageBusy_ = true;
    api->getInferenceStorage();
}

void ModelLibrary::receiveStorage(const Worker &worker, const openapi::InferenceStoragePlan &plan) {
    storageBusy_ = false;
    storageRequest_.cancel();
    storageFailed_ = worker.getHttpResponseCode() != 200 || !plan.isValid();
    storage_ = storageFailed_ ? openapi::InferenceStoragePlan{} : plan;
    rebuildEntries();
    emit changed();
}

QStringList ModelLibrary::installedProfiles() const {
    QStringList profiles;
    for (const auto &model : catalog_) {
        if (model.getState() == QStringLiteral("installed")) {
            profiles.append(model.getProfile());
        }
    }
    return profiles;
}

LibraryModel ModelLibrary::entry(const QString &profile) const {
    const auto found = std::ranges::find(catalog_, profile, &openapi::ModelDownload::getProfile);
    return found == catalog_.end() ? LibraryModel{} : LibraryModel(*found, {});
}

void ModelLibrary::rebuildEntries() {
    const QString term = query_.trimmed();
    QList<LibraryModel> downloads;
    QHash<QString, QList<LibraryModel>> grouped;
    const auto plans = storage_.getModels();
    for (const auto &download : catalog_) {
        const auto plan =
            std::ranges::find(plans, download.getProfile(), &openapi::ModelStoragePlan::getProfile);
        const LibraryModel row(download, plan == plans.end() ? openapi::ModelStoragePlan{} : *plan);
        if (row.inDownloads()) {
            downloads.append(row);
        } else if (row.matches(term)) {
            grouped[row.kind()].append(row);
        }
    }
    downloads_.replace(std::move(downloads));
    const auto replace = [&grouped](LibraryEntries &target, const QString &kind) {
        auto rows = grouped.take(kind);
        std::stable_partition(rows.begin(), rows.end(),
                              [](const LibraryModel &row) { return row.installed(); });
        target.replace(std::move(rows));
    };
    replace(answerModels_, QStringLiteral("generation"));
    replace(embeddingModels_, QStringLiteral("embedding"));
    replace(otherModels_, QString{});
    rebuildSetupChoices();
}

void ModelLibrary::rebuildSetupChoices() {
    QList<LibraryModel> answers;
    QList<LibraryModel> embeddings;
    for (const auto &download : catalog_) {
        const LibraryModel model(download, {});
        const auto kind = model.kind();
        const auto role = model.setupRole();
        if (kind == QStringLiteral("generation") &&
            (model.recommendedForSetup() || role == QStringLiteral("alternativeAnswer"))) {
            answers.append(model);
        } else if (kind == QStringLiteral("embedding") &&
                   role == QStringLiteral("optionalEmbedding")) {
            embeddings.append(model);
        }
    }
    std::stable_partition(answers.begin(), answers.end(),
                          [](const LibraryModel &row) { return row.recommendedForSetup(); });
    setupAnswers_.replace(std::move(answers));
    setupEmbeddings_.replace(std::move(embeddings));
}

bool ModelLibrary::canIssueCommand() const {
    return poll_.isActive() && catalogReady_ && !catalogFailed_ && command_ == CommandState::Idle &&
           !removal_.isValid();
}

Api *ModelLibrary::beginCommand(const QString &profile, CommandState state) {
    const auto session = backend_.session();
    if (!session) {
        return nullptr;
    }
    pendingProfile_ = profile;
    command_ = state;
    commandFeedback_.clear();
    feedbackProfile_.clear();
    emit changed();
    return commandRequest_.begin(*session);
}

void ModelLibrary::act(const LibraryModel &selected) {
    const auto profile = selected.profile();
    const auto action = selected.action();
    if (!canIssueCommand() || action == Action::NoAction || entry(profile).action() != action) {
        return;
    }
    const auto state =
        action == Action::Remove ? CommandState::PreparingRemoval : CommandState::Writing;
    if (auto *api = beginCommand(profile, state)) {
        if (action == Action::Remove) {
            sendRemovalPlan(*api, profile);
        } else if (action == Action::Download) {
            sendDownload(*api, profile);
        } else {
            sendTransferControl(*api, profile, action);
        }
    }
}

void ModelLibrary::sendDownload(Api &api, const QString &profile) {
    connect(&api, &Api::requestModelDownloadSignalFull, this,
            [this](const Worker *worker) { commandAccepted(*worker, 202); });
    connect(&api, &Api::requestModelDownloadSignalErrorFull, this,
            [this](const Worker *worker) { commandRejected(*worker); });
    openapi::ModelDownloadRequest request;
    request.setProfile(profile);
    api.requestModelDownload(request);
}

void ModelLibrary::sendTransferControl(Api &api, const QString &profile, Action action) {
    const bool pause = action == Action::Pause;
    connect(&api, pause ? &Api::pauseModelDownloadSignalFull : &Api::resumeModelDownloadSignalFull,
            this, [this](const Worker *worker) { commandAccepted(*worker, 202); });
    connect(&api,
            pause ? &Api::pauseModelDownloadSignalErrorFull
                  : &Api::resumeModelDownloadSignalErrorFull,
            this, [this](const Worker *worker) { commandRejected(*worker); });
    const auto control = pause ? &Api::pauseModelDownload : &Api::resumeModelDownload;
    (api.*control)(profile);
}

void ModelLibrary::discard(const QString &profile) {
    if (!canIssueCommand() || !entry(profile).canDiscard()) {
        return;
    }
    if (auto *api = beginCommand(profile, CommandState::Writing)) {
        connect(api, &Api::discardModelPartialSignalFull, this,
                [this](const Worker *worker) { commandAccepted(*worker, 204); });
        connect(api, &Api::discardModelPartialSignalErrorFull, this,
                [this](const Worker *worker) { commandRejected(*worker); });
        api->discardModelPartial(profile);
    }
}

void ModelLibrary::sendRemovalPlan(Api &api, const QString &profile) {
    connect(&api, &Api::readModelRemovalPlanSignalFull, this,
            [this](const Worker *worker, const openapi::ModelRemovalPlan &plan) {
                finishRemovalPlan(*worker, plan);
            });
    connect(&api, &Api::readModelRemovalPlanSignalErrorFull, this,
            [this](const Worker *worker) { commandRejected(*worker); });
    api.readModelRemovalPlan(profile);
}

void ModelLibrary::finishRemovalPlan(const Worker &worker, const openapi::ModelRemovalPlan &plan) {
    if (worker.getHttpResponseCode() != 200 || !plan.isValid() ||
        plan.getProfile() != pendingProfile_ || plan.getIdentity().isEmpty() ||
        plan.getBytes() < 0) {
        commandRejected(worker);
        return;
    }
    endCommand();
    removal_ = plan;
    emit changed();
}

void ModelLibrary::cancelRemoval() {
    if (removal_.isValid()) {
        removal_ = {};
        staleRemoval_ = false;
        emit changed();
    }
}

void ModelLibrary::confirmRemoval() {
    if (!poll_.isActive() || !removal_.isValid() || command_ != CommandState::Idle) {
        return;
    }
    const auto plan = std::exchange(removal_, {});
    staleRemoval_ = false;
    if (auto *api = beginCommand(plan.getProfile(), CommandState::Writing)) {
        connect(api, &Api::deleteModelDownloadSignalFull, this,
                [this](const Worker *worker) { commandAccepted(*worker, 204); });
        connect(api, &Api::deleteModelDownloadSignalErrorFull, this,
                [this](const Worker *worker) { removalRejected(*worker); });
        openapi::ModelRemovalRequest confirmation;
        confirmation.setIdentity(plan.getIdentity());
        api->deleteModelDownload(plan.getProfile(), openapi::OptionalParam(confirmation));
    } else {
        emit changed();
    }
}

void ModelLibrary::commandAccepted(const Worker &worker, int expectedStatus) {
    if (worker.getHttpResponseCode() != expectedStatus) {
        commandRejected(worker);
        return;
    }
    if (poll_.isActive()) {
        commandRequest_.cancel();
        command_ = CommandState::AwaitingCatalog;
        cancelReads();
        refresh();
    } else {
        endCommand();
    }
    emit changed();
}

void ModelLibrary::commandRejected(const Worker &worker) {
    feedbackProfile_ = pendingProfile_;
    commandFeedback_ = LocalApiRequest::rejection(worker, tr("Could not update the model."));
    staleRemoval_ = false;
    endCommand();
    emit changed();
}

void ModelLibrary::removalRejected(const Worker &worker) {
    const openapi::Problem problem(QString::fromUtf8(worker.response));
    if (problem.getCode() != QStringLiteral("stale_model_removal")) {
        commandRejected(worker);
        return;
    }
    const auto profile = pendingProfile_;
    endCommand();
    if (!poll_.isActive()) {
        emit changed();
        return;
    }
    if (auto *api = beginCommand(profile, CommandState::PreparingRemoval)) {
        staleRemoval_ = true;
        sendRemovalPlan(*api, profile);
    } else {
        emit changed();
    }
}

void ModelLibrary::endCommand() {
    commandRequest_.cancel();
    command_ = CommandState::Idle;
    pendingProfile_.clear();
}

} // namespace ctxfab
