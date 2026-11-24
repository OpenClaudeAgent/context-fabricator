#include "EmbeddingIndex.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "LibraryModel.h"

#include <QLocale>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
constexpr int PollIntervalMs = 1000;

bool validSnapshot(const openapi::EmbeddingIndexState &snapshot) {
    return snapshot.isValid() && !snapshot.getProfile().isEmpty() && snapshot.getEpoch() >= 0;
}

} // namespace

EmbeddingIndex::EmbeddingIndex(BackendSupervisor &backend) : backend_(backend) {
    poll_.setInterval(PollIntervalMs);
    connect(&poll_, &QTimer::timeout, this, &EmbeddingIndex::refresh);
    connect(&backend_, &BackendSupervisor::changed, this, &EmbeddingIndex::reset);
}

bool EmbeddingIndex::writing() const {
    return pending_ == Request::Toggle || pending_ == Request::Remove;
}

bool EmbeddingIndex::canWrite() const { return poll_.isActive() && loaded() && !writing(); }

QString EmbeddingIndex::feedback() const {
    return commandError_.isEmpty() ? readError_ : commandError_;
}

QString EmbeddingIndex::removalSummary() const {
    if (!removal_) {
        return {};
    }
    return tr("Removed %1 vectors (%2 of logical data). Disk space reclaimed may differ.")
        .arg(QLocale().toString(removal_->vectors), formatArtifactSize(removal_->bytes));
}

void EmbeddingIndex::setObserved(bool observed) {
    if (observed) {
        poll_.start();
        refresh();
    } else {
        poll_.stop();
        removal_.reset();
        if (pending_ == Request::Observe) {
            finish();
        } else {
            emit changed();
        }
    }
}

void EmbeddingIndex::reset() {
    snapshot_ = {};
    removal_.reset();
    commandError_.clear();
    readError_.clear();
    finish();
    refresh();
}

Api *EmbeddingIndex::begin(Request request) {
    const auto session = backend_.session();
    if (!session) {
        return nullptr;
    }
    auto *api = request_.begin(*session);
    pending_ = request;
    if (writing()) {
        commandError_.clear();
        removal_.reset();
        emit changed();
    }
    return api;
}

void EmbeddingIndex::refresh() {
    if (!poll_.isActive() || loading()) {
        return;
    }
    if (auto *api = begin(Request::Observe)) {
        connect(api, &Api::readEmbeddingIndexSignalFull, this,
                [this](const Worker *worker, const openapi::EmbeddingIndexState &state) {
                    receive(*worker, state);
                });
        connect(api, &Api::readEmbeddingIndexSignalErrorFull, this,
                [this](const Worker *worker) { reject(*worker); });
        api->readEmbeddingIndex();
    }
}

void EmbeddingIndex::setEnabled(bool enabled) {
    if (!canWrite() || enabled == this->enabled()) {
        return;
    }
    if (auto *api = begin(Request::Toggle)) {
        connect(api, &Api::setEmbeddingIndexEnabledSignalFull, this,
                [this](const Worker *worker, const openapi::EmbeddingIndexState &state) {
                    receive(*worker, state);
                });
        connect(api, &Api::setEmbeddingIndexEnabledSignalErrorFull, this,
                [this](const Worker *worker) { reject(*worker); });
        openapi::EmbeddingIndexUpdate update;
        update.setEnabled(enabled);
        api->setEmbeddingIndexEnabled(update);
    }
}

void EmbeddingIndex::remove() {
    if (!canWrite()) {
        return;
    }
    if (auto *api = begin(Request::Remove)) {
        connect(api, &Api::deleteEmbeddingIndexSignalFull, this,
                [this](const Worker *worker, const openapi::EmbeddingIndexDeletion &response) {
                    if (!response.isValid() || response.isEnabled() ||
                        response.getRemovedVectors() < 0 || response.getRemovedBytes() < 0) {
                        reject(*worker);
                        return;
                    }
                    openapi::EmbeddingIndexState state;
                    state.fromJsonObject(response.asJsonObject());
                    receive(*worker, state,
                            Removal{response.getRemovedVectors(), response.getRemovedBytes()});
                });
        connect(api, &Api::deleteEmbeddingIndexSignalErrorFull, this,
                [this](const Worker *worker) { reject(*worker); });
        api->deleteEmbeddingIndex();
    }
}

void EmbeddingIndex::receive(const Worker &worker, const openapi::EmbeddingIndexState &state,
                             std::optional<Removal> removal) {
    if (worker.getHttpResponseCode() != 200 || !validSnapshot(state)) {
        reject(worker);
        return;
    }
    const bool sameProfile = state.getProfile() == snapshot_.getProfile();
    if (sameProfile && state.getEpoch() < epoch()) {
        finish();
        return;
    }
    const bool committed = writing();
    if (committed || !sameProfile) {
        removal_ = removal;
    }
    snapshot_ = state;
    readError_.clear();
    finish();
    if (committed) {
        emit updated();
    }
}

void EmbeddingIndex::reject(const Worker &worker) {
    if (writing()) {
        commandError_ =
            LocalApiRequest::rejection(worker, tr("Could not update the embedding index."));
    } else {
        readError_ = LocalApiRequest::rejection(worker, tr("Could not read the embedding index."));
    }
    finish();
}

void EmbeddingIndex::finish() {
    request_.cancel();
    pending_ = Request::None;
    emit changed();
}

} // namespace ctxfab
