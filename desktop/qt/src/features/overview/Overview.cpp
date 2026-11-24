#include "Overview.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QLocale>

namespace ctxfab {

namespace {

QString optionalCount(bool available, int count) {
    return available ? QLocale().toString(count) : QString{};
}

} // namespace

Overview::Overview(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &Overview::backendChanged);
}

bool Overview::managed() const { return backend_.managed(); }

bool Overview::canRestart() const { return backend_.canRestart(); }

QString Overview::serviceState() const {
    return serviceState_ ? tr(serviceState_) : backend_.error();
}

QString Overview::embeddingState() const {
    return embeddingState_ ? tr(embeddingState_) : QString{};
}

QString Overview::embeddingProgress() const {
    if (!embeddingCounts_) {
        return {};
    }
    const QLocale locale;
    const auto [indexed, total] = *embeddingCounts_;
    return tr("%1 / %2 passages").arg(locale.toString(indexed), locale.toString(total));
}

void Overview::backendChanged() {
    using State = BackendSupervisor::State;
    request_.cancel();
    busy_ = false;
    ready_ = false;
    switch (backend_.state()) {
    case State::Waiting:
        finish(QT_TR_NOOP("Waiting for the local backend"));
        break;
    case State::Starting:
    case State::Restarting:
        busy_ = true;
        serviceState_ = QT_TR_NOOP("Starting the local backend…");
        emit changed();
        break;
    case State::Available:
        refresh();
        break;
    case State::Failed:
        finish(nullptr);
        break;
    }
}

void Overview::refresh() {
    if (busy_) {
        return;
    }
    const auto session = backend_.session();
    if (!session) {
        finish(backend_.state() == BackendSupervisor::State::Failed
                   ? nullptr
                   : QT_TR_NOOP("Backend session unavailable"));
        return;
    }

    busy_ = true;
    serviceState_ = QT_TR_NOOP("Connecting to the local backend…");

    auto *api = request_.begin(*session);
    connect(api, &openapi::DefaultApi::readStatusSignalFull, this,
            [this](const openapi::HttpRequestWorker *worker, const openapi::Status &status) {
                if (worker->getHttpResponseCode() != 200 || !status.isValid() ||
                    status.getApiVersion() != 1 || status.getStatus() != QStringLiteral("ready")) {
                    finish(QT_TR_NOOP("Invalid response from the backend"));
                    return;
                }
                acceptStatus(status);
                finish(QT_TR_NOOP("Ready"), true);
            });
    connect(api, &openapi::DefaultApi::readStatusSignalErrorFull, this,
            [this] { finish(QT_TR_NOOP("Backend request failed")); });
    api->readStatus();
    emit changed();
}

void Overview::restart() { backend_.restart(); }

void Overview::acceptStatus(const openapi::Status &status) {
    indexedDocuments_ = status.getIndexedDocuments();
    collectedDocuments_ =
        optionalCount(status.is_collected_documents_Set(), status.getCollectedDocuments());
    connectedSources_ = status.getConnectedSources();
    outboundAttempts_ = status.getOutboundAttempts();
    pendingOutcomes_ = optionalCount(status.is_outbound_pending_Set(), status.getOutboundPending());
    uncertainOutcomes_ =
        optionalCount(status.is_outbound_uncertain_Set(), status.getOutboundUncertain());

    embeddingState_ = nullptr;
    embeddingCounts_.reset();
    const auto retrieval = status.getRetrieval();
    if (!status.is_retrieval_Set() || !retrieval.isValid()) {
        return;
    }
    const auto state = retrieval.getState();
    if (state == QStringLiteral("notConfigured")) {
        embeddingState_ = QT_TR_NOOP("Not configured");
        return;
    }
    embeddingCounts_.emplace(retrieval.getIndexed(), retrieval.getTotal());
    if (state == QStringLiteral("ready")) {
        embeddingState_ = QT_TR_NOOP("Ready");
    } else if (state == QStringLiteral("disabled")) {
        embeddingState_ = QT_TR_NOOP("Off");
    } else if (state == QStringLiteral("incomplete")) {
        embeddingState_ = QT_TR_NOOP("Indexing");
    } else {
        embeddingState_ = QT_TR_NOOP("Unknown");
    }
}

void Overview::finish(const char *serviceState, bool ready) {
    request_.cancel();
    busy_ = false;
    ready_ = ready;
    serviceState_ = serviceState;
    emit changed();
}

} // namespace ctxfab
