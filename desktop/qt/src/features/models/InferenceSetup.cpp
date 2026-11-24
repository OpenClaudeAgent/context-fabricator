#include "InferenceSetup.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "LibraryModel.h"

#include <QDir>
#include <QRegularExpression>
#include <QUuid>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
using Phase = openapi::InferenceSetupPhase::eInferenceSetupPhase;
using Source = openapi::InferenceSetupRuntimeSource::eInferenceSetupRuntimeSource;
using Selection = openapi::InferenceSetupSelection;
constexpr int StatusIntervalMs = 1000;
constexpr int CommandTimeoutMs = 90'000;

bool validArchive(const QString &path) {
    return QDir::isAbsolutePath(path) && path.toUtf8().size() <= 2048 &&
           !path.contains(QChar::Null);
}

bool matchesIntent(const Selection &resolved, const Selection &requested) {
    return resolved.isValid() && resolved.getAnswerProfile() == requested.getAnswerProfile() &&
           resolved.isEmbedding() == requested.isEmbedding() &&
           resolved.getRuntimeSource().getValue() == requested.getRuntimeSource().getValue() &&
           (!requested.is_embedding_profile_Set() ||
            resolved.getEmbeddingProfile() == requested.getEmbeddingProfile());
}

bool validPlan(const openapi::InferenceSetupPlan &plan, const Selection &requested) {
    static const QRegularExpression fingerprint(QStringLiteral("^[0-9a-f]{64}\\z"));
    return plan.isValid() && matchesIntent(plan.getSelection(), requested) &&
           fingerprint.match(plan.getFingerprint()).hasMatch() &&
           plan.getPeakAdditionalBytes() >= 0 && plan.getMaximumTransferBytes() >= 0 &&
           plan.getAvailableBytes() >= 0;
}

bool validStatus(const openapi::InferenceSetupStatus &status) {
    const auto phase = status.getPhase().getValue();
    return status.isValid() && !QUuid(status.getOperationId()).isNull() &&
           phase >= Phase::RUNTIME && phase <= Phase::INTERRUPTED;
}

} // namespace

InferenceSetup::InferenceSetup(BackendSupervisor &backend) : backend_(backend) {
    poll_.setInterval(StatusIntervalMs);
    connect(&poll_, &QTimer::timeout, this, &InferenceSetup::pollStatus);
    connect(&backend_, &BackendSupervisor::changed, this, &InferenceSetup::reset);
}

bool InferenceSetup::waitingForArchive() const {
    return status_.getPhase().getValue() == Phase::WAITINGFORARCHIVE;
}

bool InferenceSetup::transferring() const {
    const auto phase = status_.getPhase().getValue();
    return phase == Phase::ANSWERWEIGHTS || phase == Phase::EMBEDDINGWEIGHTS;
}

bool InferenceSetup::inProgress() const {
    const auto phase = status_.getPhase().getValue();
    return transferring() || waitingForArchive() || phase == Phase::RUNTIME ||
           phase == Phase::ACTIVATINGANSWER;
}

bool InferenceSetup::canManageDownloads() const {
    const auto phase = status_.getPhase().getValue();
    return transferring() || phase == Phase::FAILED || phase == Phase::INTERRUPTED;
}

bool InferenceSetup::writing() const {
    return requestState_ == Request::Confirm || requestState_ == Request::ProvideArchive ||
           requestState_ == Request::CancelArchive;
}

QString InferenceSetup::activeProfile() const {
    const auto phase = status_.getPhase().getValue();
    const bool stopped = phase == Phase::FAILED || phase == Phase::INTERRUPTED;
    return phase == Phase::EMBEDDINGWEIGHTS || (stopped && answerReady() && lastEmbedding())
               ? lastEmbeddingProfile()
               : lastAnswerProfile();
}

QString InferenceSetup::statusText() const {
    switch (status_.getPhase().getValue()) {
    case Phase::WAITINGFORARCHIVE:
        return tr("Choose the runtime ZIP again to continue setup.");
    case Phase::RUNTIME:
        return tr("Setting up the runtime…");
    case Phase::ANSWERWEIGHTS:
        return tr("Downloading answer weights…");
    case Phase::ACTIVATINGANSWER:
        return tr("Activating the answer model…");
    case Phase::EMBEDDINGWEIGHTS:
        return tr("Downloading embedding weights…");
    default:
        return outcomeText();
    }
}

QString InferenceSetup::outcomeText() const {
    switch (status_.getPhase().getValue()) {
    case Phase::COMPLETED:
        return answerReady() ? tr("Setup completed. Answers are ready.") : tr("Setup completed.");
    case Phase::FAILED:
        return answerReady() ? tr("Setup stopped. Answers are ready.") : tr("Setup failed.");
    case Phase::INTERRUPTED:
        return answerReady() ? tr("Setup interrupted. Answers are ready.")
                             : tr("Setup interrupted.");
    default:
        return {};
    }
}

QString InferenceSetup::feedback() const {
    return commandFeedback_.isEmpty() ? statusFeedback_ : commandFeedback_;
}

bool InferenceSetup::planNeedsArchive() const {
    return planRuntimeRequired() &&
           plan_.getSelection().getRuntimeSource().getValue() == Source::LOCALARCHIVE;
}

bool InferenceSetup::planAllowed() const {
    return hasPlan() && plan_.isCanStart() && !inProgress() && !writing();
}

QString InferenceSetup::planTransfer() const {
    return formatArtifactSize(plan_.getMaximumTransferBytes());
}
QString InferenceSetup::planPeak() const {
    return formatArtifactSize(plan_.getPeakAdditionalBytes());
}
QString InferenceSetup::planAvailable() const {
    return formatArtifactSize(plan_.getAvailableBytes());
}

QString InferenceSetup::planBlocker() const {
    const auto code = plan_.getBlocker();
    if (plan_.isCanStart()) {
        return {};
    }
    if (code == QStringLiteral("runtime_unavailable")) {
        return tr("The local inference engine is unavailable.");
    }
    if (code == QStringLiteral("runtime_busy")) {
        return tr("The local inference engine is being installed.");
    }
    if (code == QStringLiteral("model_job_busy")) {
        return tr("A selected model download is already in progress or paused.");
    }
    if (code == QStringLiteral("runtime_download_unavailable")) {
        return tr("The local inference engine cannot be downloaded.");
    }
    if (code == QStringLiteral("storage_limit")) {
        return tr("There is not enough available storage for this setup.");
    }
    return tr("Setup is unavailable. Refresh the estimate and try again.");
}

bool InferenceSetup::canConfirm(const QUrl &archive) const {
    return planAllowed() && (!planNeedsArchive() || validArchive(archive.toLocalFile()));
}

void InferenceSetup::setObserved(bool observed) {
    observed_ = observed;
    updateObservation();
    if (observed_) {
        pollStatus();
    }
}

void InferenceSetup::reset() {
    request_.cancel();
    requestState_ = Request::Idle;
    expectedOperation_.clear();
    plan_ = {};
    status_ = {};
    commandFeedback_.clear();
    statusFeedback_.clear();
    statusKnown_ = false;
    updateObservation();
    emit changed();
    pollStatus();
}

void InferenceSetup::updateObservation() {
    if (observed_ || inProgress() || writing()) {
        if (!poll_.isActive()) {
            poll_.start();
        }
    } else {
        poll_.stop();
        if (requestState_ == Request::Status) {
            request_.cancel();
            requestState_ = Request::Idle;
        }
    }
}

Api *InferenceSetup::begin(Request state) {
    const auto session = backend_.session();
    if (!session) {
        return nullptr;
    }
    requestState_ = state;
    if (state != Request::Status) {
        commandFeedback_.clear();
    }
    return request_.begin(*session, state == Request::Status ? std::nullopt
                                                             : std::optional{CommandTimeoutMs});
}

void InferenceSetup::pollStatus() {
    if (!poll_.isActive() || requestState_ != Request::Idle) {
        return;
    }
    if (auto *api = begin(Request::Status)) {
        connect(api, &Api::getInferenceSetupStatusSignalFull, this,
                [this](const Worker *worker, const openapi::InferenceSetupStatus &status) {
                    receiveStatus(*worker, status);
                });
        connect(api, &Api::getInferenceSetupStatusSignalErrorFull, this,
                [this](const Worker *worker) { reject(*worker); });
        api->getInferenceSetupStatus();
    }
}

bool InferenceSetup::canReview() const {
    return observed_ && statusKnown_ && !inProgress() && !writing() && !reviewing();
}

void InferenceSetup::review(const QString &answer, bool embedding, const QString &embeddingProfile,
                            bool useLocalArchive) {
    if (!canReview() || answer.isEmpty()) {
        return;
    }
    auto *api = begin(Request::Review);
    if (!api) {
        return;
    }
    plan_ = {};
    Selection selection;
    selection.setAnswerProfile(answer);
    selection.setEmbedding(embedding);
    if (embedding && !embeddingProfile.isEmpty()) {
        selection.setEmbeddingProfile(embeddingProfile);
    }
    openapi::InferenceSetupRuntimeSource source;
    source.setValue(useLocalArchive ? Source::LOCALARCHIVE : Source::HOSTED);
    selection.setRuntimeSource(source);
    connect(api, &Api::getInferenceSetupPlanSignalFull, this,
            [this, selection](const Worker *worker, const openapi::InferenceSetupPlan &plan) {
                receivePlan(*worker, plan, selection);
            });
    connect(api, &Api::getInferenceSetupPlanSignalErrorFull, this,
            [this](const Worker *worker) { reject(*worker); });
    api->getInferenceSetupPlan(selection);
    emit changed();
}

void InferenceSetup::receivePlan(const Worker &worker, const openapi::InferenceSetupPlan &plan,
                                 const Selection &requested) {
    if (worker.getHttpResponseCode() == 200 && validPlan(plan, requested)) {
        plan_ = plan;
        finish();
    } else {
        reject(worker);
    }
}

void InferenceSetup::discardPlan() {
    if (reviewing()) {
        request_.cancel();
        requestState_ = Request::Idle;
    }
    plan_ = {};
    commandFeedback_.clear();
    emit changed();
}

void InferenceSetup::confirm(const QUrl &archive) {
    if (!canConfirm(archive)) {
        return;
    }
    auto *api = begin(Request::Confirm);
    if (!api) {
        return;
    }
    expectedOperation_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
    openapi::InferenceSetupConfirmation confirmation;
    confirmation.setOperationId(expectedOperation_);
    confirmation.setSelection(plan_.getSelection());
    confirmation.setFingerprint(plan_.getFingerprint());
    if (planNeedsArchive()) {
        confirmation.setArchivePath(archive.toLocalFile());
    }
    plan_ = {};
    watchWrite(*api);
    api->startInferenceSetup(confirmation);
    emit changed();
}

bool InferenceSetup::matchesWaitingOperation(const QString &operation) const {
    return waitingForArchive() && !writing() && operationId() == operation;
}

void InferenceSetup::provideArchive(const QString &operation, const QUrl &archive) {
    if (!matchesWaitingOperation(operation)) {
        return;
    }
    const auto path = archive.toLocalFile();
    if (!validArchive(path)) {
        commandFeedback_ = tr("Choose a local runtime archive.");
        emit changed();
        return;
    }
    if (auto *api = begin(Request::ProvideArchive)) {
        expectedOperation_ = operation;
        openapi::InferenceSetupArchive selection;
        selection.setOperationId(operation);
        selection.setArchivePath(path);
        watchWrite(*api);
        api->startInferenceSetupArchive(selection);
        emit changed();
    }
}

void InferenceSetup::cancelArchive(const QString &operation) {
    if (!matchesWaitingOperation(operation)) {
        return;
    }
    if (auto *api = begin(Request::CancelArchive)) {
        expectedOperation_ = operation;
        watchWrite(*api);
        api->cancelInferenceSetupArchive(operation);
        emit changed();
    }
}

void InferenceSetup::watchWrite(Api &api) {
    const auto success = [this](const Worker *worker, const openapi::InferenceSetupStatus &status) {
        receiveStatus(*worker, status);
    };
    const auto failure = [this](const Worker *worker) { reject(*worker); };
    switch (requestState_) {
    case Request::Confirm:
        connect(&api, &Api::startInferenceSetupSignalFull, this, success);
        connect(&api, &Api::startInferenceSetupSignalErrorFull, this, failure);
        break;
    case Request::ProvideArchive:
        connect(&api, &Api::startInferenceSetupArchiveSignalFull, this, success);
        connect(&api, &Api::startInferenceSetupArchiveSignalErrorFull, this, failure);
        break;
    case Request::CancelArchive:
        connect(&api, &Api::cancelInferenceSetupArchiveSignalFull, this, success);
        connect(&api, &Api::cancelInferenceSetupArchiveSignalErrorFull, this, failure);
        break;
    default:
        Q_UNREACHABLE();
    }
}

void InferenceSetup::receiveStatus(const Worker &worker,
                                   const openapi::InferenceSetupStatus &status) {
    const int expected =
        requestState_ == Request::Status || requestState_ == Request::CancelArchive ? 200 : 202;
    if (worker.getHttpResponseCode() != expected || !validStatus(status) ||
        (writing() && QUuid(status.getOperationId()) != QUuid(expectedOperation_))) {
        reject(worker);
        return;
    }
    publishStatus(status);
}

void InferenceSetup::publishStatus(const openapi::InferenceSetupStatus &status) {
    const auto phaseChanged = status.getPhase().getValue() != status_.getPhase().getValue();
    const auto operationChanged = status.getOperationId() != operationId();
    const bool modelsUpdated =
        status.isAnswerReady() && (operationChanged || phaseChanged || !answerReady());
    status_ = status;
    statusKnown_ = true;
    statusFeedback_.clear();
    if (inProgress()) {
        plan_ = {};
    }
    finish();
    if (modelsUpdated) {
        emit modelsChanged();
    }
}

void InferenceSetup::reject(const Worker &worker) {
    if (requestState_ == Request::Status) {
        if (worker.getHttpResponseCode() == 404) {
            statusKnown_ = true;
            status_ = {};
            statusFeedback_.clear();
        } else {
            statusFeedback_ =
                LocalApiRequest::rejection(worker, tr("Unable to check the setup status."));
        }
    } else {
        commandFeedback_ = LocalApiRequest::rejection(
            worker,
            reviewing() ? tr("Unable to check the setup requirements.") : tr("Setup failed."));
    }
    finish();
}

void InferenceSetup::finish() {
    requestState_ = Request::Idle;
    expectedOperation_.clear();
    updateObservation();
    emit changed();
}

} // namespace ctxfab
