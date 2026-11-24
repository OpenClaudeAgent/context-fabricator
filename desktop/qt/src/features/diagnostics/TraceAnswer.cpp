#include "TraceAnswer.h"

#include "Answers.h"
#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "HistoryPreferences.h"

#include <QDateTime>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
constexpr qint64 RecheckIntervalMs = 3'600'000;

} // namespace

TraceAnswer::TraceAnswer(BackendSupervisor &backend, Answers &answers,
                         HistoryPreferences &preferences)
    : backend_(backend), answers_(answers), preferences_(preferences) {
    expiryTimer_.setSingleShot(true);
    connect(&expiryTimer_, &QTimer::timeout, this, &TraceAnswer::checkExpiry);
    connect(&answers_, &Answers::changed, this, &TraceAnswer::synchronize);
    connect(&backend_, &BackendSupervisor::changed, this, [this] { inspect({}, false); });
    connect(&preferences_, &HistoryPreferences::historyCleared, this, &TraceAnswer::forgetSaved);
    connect(&preferences_, &HistoryPreferences::retentionReduced, this, &TraceAnswer::forgetSaved);
}

bool TraceAnswer::live() const {
    return !traceId_.isEmpty() && answers_.traceId() == traceId_ && answers_.result().present();
}

QString TraceAnswer::sessionQuestion() const { return answers_.questionForTrace(traceId_); }

bool TraceAnswer::available() const {
    return !traceId_.isEmpty() && (knownAnswer_ || live() || !sessionQuestion().isEmpty());
}

QString TraceAnswer::question() const {
    const auto session = sessionQuestion();
    if (!session.isEmpty()) {
        return session;
    }
    return live() ? details().question() : saved_.question();
}

QString TraceAnswer::feedback() const { return tr(qUtf8Printable(feedback_)); }

AnswerResult TraceAnswer::result() const { return live() ? answers_.result() : saved_.result(); }

TraceAnswerDetails TraceAnswer::details() const {
    return live() ? TraceAnswerDetails(answers_.result().payload()) : TraceAnswerDetails{};
}

void TraceAnswer::inspect(const QString &traceId, bool isAnswerTrace) {
    if (traceId_ != traceId) {
        clearSaved();
        traceId_ = traceId;
        knownAnswer_ = false;
        requested_ = false;
    }
    knownAnswer_ = knownAnswer_ || isAnswerTrace;
    synchronize();
}

void TraceAnswer::synchronize() {
    if (available() && !live() && !requested_) {
        fetch();
    }
    emit changed();
}

void TraceAnswer::clearSaved() {
    request_.cancel();
    expiryTimer_.stop();
    saved_ = {};
    expiresAt_ = 0;
    loading_ = false;
    feedback_.clear();
}

void TraceAnswer::forgetSaved() {
    clearSaved();
    emit changed();
}

void TraceAnswer::reload() {
    clearSaved();
    requested_ = false;
    synchronize();
}

void TraceAnswer::revalidateOnActivation(Qt::ApplicationState state) {
    if (state == Qt::ApplicationActive && saved_.present() && !live()) {
        reload();
    }
}

void TraceAnswer::fetch() {
    requested_ = true;
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Could not load the saved answer."));
        return;
    }
    auto *api = request_.begin(*session);
    connect(api, &Api::getAnswerHistoryEntryByTraceSignalFull, this,
            [this](const Worker *worker, const openapi::AnswerHistoryEntry &entry) {
                receive(*worker, entry);
            });
    connect(api, &Api::getAnswerHistoryEntryByTraceSignalErrorFull, this,
            [this](const Worker *worker) {
                finish(worker->getHttpResponseCode() == 404
                           ? QString{}
                           : QT_TR_NOOP("Could not load the saved answer."));
            });
    loading_ = true;
    api->getAnswerHistoryEntryByTrace(traceId_);
}

void TraceAnswer::receive(const Worker &worker, const openapi::AnswerHistoryEntry &entry) {
    if (worker.getHttpResponseCode() != 200 || !entry.isValid() || entry.getId() <= 0 ||
        entry.getTraceId() != traceId_) {
        finish(QT_TR_NOOP("Could not load the saved answer."));
        return;
    }
    const SavedAnswer candidate(entry);
    expiresAt_ = candidate.expiresAtMs(preferences_.retentionDays());
    if (expiresAt_ > QDateTime::currentMSecsSinceEpoch()) {
        saved_ = candidate;
        checkExpiry();
    }
    finish({});
}

void TraceAnswer::finish(const QString &message) {
    request_.cancel();
    loading_ = false;
    feedback_ = message;
    emit changed();
}

void TraceAnswer::checkExpiry() {
    const auto delay = expiresAt_ - QDateTime::currentMSecsSinceEpoch();
    if (delay > 0) {
        expiryTimer_.start(static_cast<int>(std::min(delay, RecheckIntervalMs)));
    } else {
        forgetSaved();
    }
}

} // namespace ctxfab
