#include "AnswerUsageModel.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "TraceRecord.h"

#include <QDateTime>
#include <QLocale>

namespace ctxfab {

QString AnswerUsageRow::client() const { return TraceRecord::clientName(group_.getClient()); }

QString AnswerUsageRow::profile() const {
    const auto profile = group_.getModelProfile();
    return profile.isEmpty() ? tr("Model unspecified") : profile;
}

QString AnswerUsageRow::median() const {
    return group_.is_p50_micros_Set() ? TraceRecord::formatDuration(group_.getP50Micros())
                                      : QString{};
}

QString AnswerUsageRow::p95() const {
    return group_.is_p95_micros_Set() ? TraceRecord::formatDuration(group_.getP95Micros())
                                      : QString{};
}

AnswerUsageModel::AnswerUsageModel(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &AnswerUsageModel::backendChanged);
}

QString AnswerUsageModel::feedback() const { return tr(qUtf8Printable(feedback_)); }

QString AnswerUsageModel::period() const {
    if (!snapshot_ || !snapshot_->is_first_at_Set() || !snapshot_->is_last_at_Set()) {
        return {};
    }
    const auto format = [](qint64 seconds) {
        return QLocale().toString(QDateTime::fromSecsSinceEpoch(seconds), QLocale::ShortFormat);
    };
    const auto first = snapshot_->getFirstAt();
    const auto last = snapshot_->getLastAt();
    return first == last ? format(first)
                         : QStringLiteral("%1 – %2").arg(format(first), format(last));
}

QList<AnswerUsageRow> AnswerUsageModel::groups() const {
    QList<AnswerUsageRow> rows;
    if (snapshot_) {
        const auto groups = snapshot_->getGroups();
        rows.reserve(groups.size());
        for (const auto &group : groups) {
            rows.append(AnswerUsageRow(group));
        }
    }
    return rows;
}

void AnswerUsageModel::setActive(bool active) {
    if (active_ == active) {
        return;
    }
    active_ = active;
    backendChanged();
}

void AnswerUsageModel::backendChanged() {
    snapshot_.reset();
    ready_ = false;
    finish({});
    refresh();
}

void AnswerUsageModel::refresh() {
    if (!active_) {
        return;
    }
    const auto session = backend_.session();
    ready_ = session.has_value();
    if (!session) {
        snapshot_.reset();
        finish(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    using Api = openapi::DefaultApi;
    using Worker = openapi::HttpRequestWorker;
    auto *api = request_.begin(*session);
    connect(api, &Api::readAnswerUsageSignalFull, this,
            [this](const Worker *worker, const openapi::AnswerUsage &data) {
                if (worker->getHttpResponseCode() != 200 || !data.isValid()) {
                    finish(QT_TR_NOOP("Invalid response from the backend"));
                    return;
                }
                snapshot_ = data;
                finish({});
            });
    connect(api, &Api::readAnswerUsageSignalErrorFull, this, [this](const Worker *worker) {
        finish(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    feedback_.clear();
    busy_ = true;
    api->readAnswerUsage();
    emit changed();
}

void AnswerUsageModel::finish(const QString &message) {
    request_.cancel();
    busy_ = false;
    feedback_ = message;
    emit changed();
}

} // namespace ctxfab
