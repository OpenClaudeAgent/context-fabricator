#include "EmbeddingProfiles.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QSet>

#include <algorithm>
#include <utility>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
constexpr int SelectionTimeoutMs = 120'000;

bool validSelection(const openapi::EmbeddingModelSelection &snapshot) {
    if (!snapshot.isValid()) {
        return false;
    }
    QSet<QString> ids;
    const auto profiles = snapshot.getProfiles();
    for (const auto &profile : profiles) {
        if (!profile.isValid() || profile.getProfile().isEmpty() ||
            ids.contains(profile.getProfile())) {
            return false;
        }
        ids.insert(profile.getProfile());
    }
    return !snapshot.is_active_profile_Set() || ids.contains(snapshot.getActiveProfile());
}

} // namespace

EmbeddingProfiles::EmbeddingProfiles(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &EmbeddingProfiles::reset);
}

bool EmbeddingProfiles::ready() const { return backend_.session().has_value(); }

QStringList EmbeddingProfiles::labels() const {
    const auto profiles = snapshot_.getProfiles();
    QStringList labels;
    labels.reserve(profiles.size());
    for (const auto &profile : profiles) {
        labels.append(profile.getLabel());
    }
    return labels;
}

int EmbeddingProfiles::selectedIndex() const {
    const auto profiles = snapshot_.getProfiles();
    const auto found = std::ranges::find(profiles, snapshot_.getActiveProfile(),
                                         &openapi::EmbeddingModelProfile::getProfile);
    return found == profiles.end() ? -1 : static_cast<int>(found - profiles.begin());
}

void EmbeddingProfiles::setObserved(bool observed) {
    observed_ = observed;
    if (observed) {
        refresh();
    } else if (requestedProfile_.isEmpty()) {
        finish();
    }
}

void EmbeddingProfiles::reset() {
    snapshot_ = {};
    feedback_.clear();
    refreshQueued_ = false;
    finish();
    refresh();
}

void EmbeddingProfiles::refresh() {
    if (!observed_) {
        return;
    }
    if (busy()) {
        refreshQueued_ = true;
    } else {
        send({});
    }
}

void EmbeddingProfiles::select(int index) {
    const auto profiles = snapshot_.getProfiles();
    if (observed_ && !busy() && index >= 0 && index < profiles.size() && index != selectedIndex()) {
        send(profiles.at(index).getProfile());
    }
}

void EmbeddingProfiles::send(const QString &profile) {
    const auto session = backend_.session();
    if (!session) {
        return;
    }
    const bool selecting = !profile.isEmpty();
    auto *api =
        request_.begin(*session, selecting ? std::optional{SelectionTimeoutMs} : std::nullopt);
    requestedProfile_ = profile;
    busy_ = true;
    feedback_.clear();
    emit changed();
    connect(api,
            selecting ? &Api::selectEmbeddingModelSignalFull : &Api::listEmbeddingModelsSignalFull,
            this, [this](const Worker *worker, const openapi::EmbeddingModelSelection &snapshot) {
                receive(*worker, snapshot);
            });
    connect(api,
            selecting ? &Api::selectEmbeddingModelSignalErrorFull
                      : &Api::listEmbeddingModelsSignalErrorFull,
            this, [this](const Worker *worker) { reject(*worker); });
    if (selecting) {
        openapi::EmbeddingModelUpdate update;
        update.setProfile(profile);
        api->selectEmbeddingModel(update);
    } else {
        api->listEmbeddingModels();
    }
}

void EmbeddingProfiles::receive(const Worker &worker,
                                const openapi::EmbeddingModelSelection &snapshot) {
    const bool selecting = !requestedProfile_.isEmpty();
    if (worker.getHttpResponseCode() != 200 || !validSelection(snapshot) ||
        (selecting && snapshot.getActiveProfile() != requestedProfile_)) {
        reject(worker);
        return;
    }
    snapshot_ = snapshot;
    finish();
    if (selecting) {
        emit selected();
    }
}

void EmbeddingProfiles::reject(const Worker &worker) {
    feedback_ = LocalApiRequest::rejection(
        worker, requestedProfile_.isEmpty() ? tr("Could not load embedding models.")
                                            : tr("Could not select the embedding model."));
    finish();
}

void EmbeddingProfiles::finish() {
    request_.cancel();
    requestedProfile_.clear();
    busy_ = false;
    emit changed();
    if (std::exchange(refreshQueued_, false)) {
        refresh();
    }
}

} // namespace ctxfab
