#include "HistoryPreferences.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

namespace ctxfab {

HistoryPreferences::HistoryPreferences(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &HistoryPreferences::backendChanged);
}

void HistoryPreferences::open() {
    if (!opened_) {
        opened_ = true;
        reload();
    }
}

void HistoryPreferences::backendChanged() {
    request_.cancel();
    busy_ = false;
    ready_ = false;
    if (opened_) {
        reload();
    }
}

openapi::DefaultApi *HistoryPreferences::begin() {
    if (busy_) {
        return nullptr;
    }
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Backend session unavailable"));
        return nullptr;
    }
    busy_ = true;
    feedback_.clear();
    return request_.begin(*session);
}

void HistoryPreferences::reload() {
    opened_ = true;
    auto *api = begin();
    if (!api) {
        return;
    }
    connect(api, &openapi::DefaultApi::getAnswerHistorySettingsSignalFull, this,
            [this](const openapi::HttpRequestWorker *worker,
                   const openapi::AnswerHistorySettings &policy) { accept(*worker, policy, {}); });
    connect(api, &openapi::DefaultApi::getAnswerHistorySettingsSignalErrorFull, this,
            [this](const openapi::HttpRequestWorker *worker) { failed(*worker); });
    api->getAnswerHistorySettings();
    emit changed();
}

void HistoryPreferences::setEnabled(bool enabled) {
    if (ready_ && !busy_ && !conflict_ && enabled != this->enabled()) {
        auto update = *policy_;
        update.setEnabled(enabled);
        save(update);
    }
}

void HistoryPreferences::setRetentionDays(int days) {
    if (!ready_ || busy_ || conflict_) {
        return;
    }
    if (days < 1 || days > 365) {
        finish(QT_TR_NOOP("Enter a duration from 1 to 365 days."));
        return;
    }
    if (days != retentionDays()) {
        auto update = *policy_;
        update.setRetentionDays(days);
        save(update);
    }
}

void HistoryPreferences::setCustomRetention(const QString &value) {
    setRetentionDays(value.toInt());
}

void HistoryPreferences::save(const openapi::AnswerHistorySettings &policy) {
    auto *api = begin();
    if (!api) {
        return;
    }
    connect(api, &openapi::DefaultApi::updateAnswerHistorySettingsSignalFull, this,
            [this](const openapi::HttpRequestWorker *worker,
                   const openapi::AnswerHistorySettings &effective) {
                accept(*worker, effective, QT_TR_NOOP("History settings saved."));
            });
    connect(api, &openapi::DefaultApi::updateAnswerHistorySettingsSignalErrorFull, this,
            [this](const openapi::HttpRequestWorker *worker) { failed(*worker); });
    api->updateAnswerHistorySettings(policy);
    emit changed();
}

void HistoryPreferences::clear() {
    if (!ready_) {
        return;
    }
    auto *api = begin();
    if (!api) {
        return;
    }
    connect(api, &openapi::DefaultApi::clearAnswerHistorySignalFull, this,
            [this](const openapi::HttpRequestWorker *worker) {
                if (worker->getHttpResponseCode() == 204) {
                    finish(QT_TR_NOOP("Answer history cleared."));
                    emit historyCleared();
                } else {
                    finish(QT_TR_NOOP("Invalid response from the backend"));
                }
            });
    connect(api, &openapi::DefaultApi::clearAnswerHistorySignalErrorFull, this,
            [this](const openapi::HttpRequestWorker *worker) { failed(*worker); });
    api->clearAnswerHistory();
    emit changed();
}

void HistoryPreferences::accept(const openapi::HttpRequestWorker &worker,
                                const openapi::AnswerHistorySettings &policy,
                                const QString &message) {
    if (worker.getHttpResponseCode() != 200 || !policy.isValid() || policy.getRevision() < 0) {
        finish(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    const bool reduced =
        !policy.isEnabled() || (policy_ && policy.getRetentionDays() < policy_->getRetentionDays());
    policy_ = policy;
    ready_ = true;
    conflict_ = false;
    finish(message);
    if (reduced) {
        emit retentionReduced();
    }
}

void HistoryPreferences::failed(const openapi::HttpRequestWorker &worker) {
    if (worker.getHttpResponseCode() == 409) {
        conflict_ = true;
        finish(QT_TR_NOOP("History settings changed. Reload before editing."));
        return;
    }
    finish(LocalApiRequest::rejection(worker, QT_TR_NOOP("Backend request failed")));
}

QString HistoryPreferences::feedback() const { return tr(qUtf8Printable(feedback_)); }

void HistoryPreferences::finish(const QString &message) {
    request_.cancel();
    busy_ = false;
    feedback_ = message;
    emit changed();
}

} // namespace ctxfab
