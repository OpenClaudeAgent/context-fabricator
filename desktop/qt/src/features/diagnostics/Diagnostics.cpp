#include "Diagnostics.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QDateTime>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocale>
#include <QSaveFile>

#include <algorithm>
#include <array>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
using Snapshot = openapi::TelemetrySnapshot;

QByteArray exportJson(const Snapshot &snapshot) {
    auto document = snapshot.asJsonObject();
    // The generated serializer omits empty arrays, including these required fields.
    for (const auto key :
         {QLatin1StringView("logs"), QLatin1StringView("metrics"), QLatin1StringView("traces")}) {
        if (!document.contains(key)) {
            document.insert(QString(key), QJsonArray{});
        }
    }
    return QJsonDocument(document).toJson();
}

bool matches(const TraceRecord &record, const QString &filter) {
    if (filter.isEmpty()) {
        return true;
    }
    const std::array fields{record.name(),        record.title(),   record.operation(),
                            record.httpRequest(), record.traceId(), record.spanId()};
    return std::ranges::any_of(fields, [&filter](const QString &field) {
        return field.contains(filter, Qt::CaseInsensitive);
    });
}

} // namespace

Diagnostics::Diagnostics(BackendSupervisor &backend) : backend_(backend) {
    expiry_.setSingleShot(true);
    connect(&expiry_, &QTimer::timeout, this, [this] {
        emit changed();
        if (visible_) {
            refresh();
        }
    });
    connect(&backend_, &BackendSupervisor::changed, this, &Diagnostics::backendChanged);
}

QString Diagnostics::feedback() const { return tr(qUtf8Printable(feedback_)); }

openapi::TelemetryHealth Diagnostics::health() const {
    return snapshot_ ? snapshot_->getHealth() : openapi::TelemetryHealth{};
}

QString Diagnostics::healthStatus() const {
    if (!ready()) {
        return {};
    }
    if (recording()) {
        return tr("Recording");
    }
    return health().getState() == "gaps" ? tr("Data gaps") : tr("Degraded");
}

bool Diagnostics::recording() const { return health().getState() == "ready"; }

bool Diagnostics::debugActive() const {
    return health().getDebugUntil() > QDateTime::currentSecsSinceEpoch();
}

QString Diagnostics::debugExpiry() const {
    return debugActive()
               ? QLocale().toString(QDateTime::fromSecsSinceEpoch(health().getDebugUntil()).time(),
                                    QLocale::ShortFormat)
               : QString{};
}

QString Diagnostics::storageUsed() const {
    return QLocale().formattedDataSize(health().getFileBytes());
}

QString Diagnostics::storageBudget() const {
    return QLocale().formattedDataSize(health().getBudgetBytes());
}

double Diagnostics::storageFraction() const {
    const auto value = health();
    const auto budget = value.getBudgetBytes();
    return budget > 0
               ? std::clamp(static_cast<double>(value.getFileBytes()) / static_cast<double>(budget),
                            0.0, 1.0)
               : 0.0;
}

QString Diagnostics::storageRetention() const {
    return tr("%1 h").arg(QLocale().toString(health().getRetentionHours()));
}

QString Diagnostics::storageOldest() const {
    const auto value = health();
    if (!value.is_oldest_at_Set()) {
        return {};
    }
    return QLocale().toString(QDateTime::fromSecsSinceEpoch(value.getOldestAt()),
                              QLocale::ShortFormat);
}

QString Diagnostics::queuedCount() const { return QLocale().toString(health().getQueued()); }
QString Diagnostics::droppedCount() const { return QLocale().toString(health().getDropped()); }
QString Diagnostics::purgedCount() const { return QLocale().toString(health().getPruned()); }

QList<TraceRecord> Diagnostics::traces() const {
    using Kind = openapi::DiagnosticRequestKind::eDiagnosticRequestKind;
    QList<TraceRecord> result;
    if (snapshot_) {
        for (const auto &sample : snapshot_->getTraces()) {
            if (!includeBackgroundChecks_ &&
                sample.getTraceRequestKind().getValue() == Kind::BACKGROUNDCHECK) {
                continue;
            }
            const TraceRecord record(sample);
            if (matches(record, filter_)) {
                result.append(record);
            }
        }
    }
    return result;
}

QList<TraceRecord> Diagnostics::metrics() const {
    QList<TraceRecord> result;
    if (snapshot_) {
        for (const auto &sample : snapshot_->getMetrics()) {
            const TraceRecord record(sample);
            if (matches(record, filter_)) {
                result.append(record);
            }
        }
    }
    return result;
}

void Diagnostics::setPageVisible(bool visible) {
    visible_ = visible;
    if (visible_) {
        refresh();
    }
}

void Diagnostics::setFilter(const QString &query) {
    const auto normalized = query.trimmed();
    if (filter_ != normalized) {
        filter_ = normalized;
        emit recordsChanged();
    }
}

void Diagnostics::setIncludeBackgroundChecks(bool include) {
    if (includeBackgroundChecks_ != include) {
        includeBackgroundChecks_ = include;
        emit recordsChanged();
    }
}

void Diagnostics::backendChanged() {
    request_.cancel();
    expiry_.stop();
    snapshot_.reset();
    busy_ = false;
    feedback_.clear();
    emit recordsChanged();
    emit changed();
    if (visible_) {
        refresh();
    }
}

Api *Diagnostics::begin() {
    if (busy_) {
        return nullptr;
    }
    const auto session = backend_.session();
    if (!session) {
        expiry_.stop();
        snapshot_.reset();
        emit recordsChanged();
        finish(QT_TR_NOOP("Backend session unavailable"));
        return nullptr;
    }
    busy_ = true;
    feedback_.clear();
    return request_.begin(*session);
}

void Diagnostics::refresh() {
    auto *api = begin();
    if (!api) {
        return;
    }
    connect(api, &Api::readTelemetrySignalFull, this,
            [this](const Worker *worker, const Snapshot &data) { accept(*worker, data, {}); });
    connect(api, &Api::readTelemetrySignalErrorFull, this, [this](const Worker *worker) {
        finish(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    api->readTelemetry();
    emit changed();
}

void Diagnostics::toggleDebug() {
    control(debugActive() ? QStringLiteral("disableDebug") : QStringLiteral("enableDebug"), {});
}

void Diagnostics::purgeExpired() {
    control(QStringLiteral("purgeExpired"), QT_TR_NOOP("Expired records purged."));
}

void Diagnostics::saveSnapshot(const QUrl &destination) {
    if (!snapshot_) {
        return;
    }
    feedback_ = QT_TR_NOOP("Couldn’t export diagnostics.");
    if (destination.isLocalFile() && destination.host().isEmpty()) {
        QSaveFile file(destination.toLocalFile());
        const auto bytes = exportJson(*snapshot_);
        if (file.open(QIODevice::WriteOnly) && file.write(bytes) == bytes.size() && file.commit()) {
            feedback_ = QT_TR_NOOP("Diagnostics exported.");
        }
    }
    emit changed();
}

void Diagnostics::control(const QString &command, const QString &message) {
    if (!ready()) {
        return;
    }
    auto *api = begin();
    if (!api) {
        return;
    }
    connect(api, &Api::controlTelemetrySignalFull, this,
            [this, message](const Worker *worker, const Snapshot &data) {
                accept(*worker, data, message, true);
            });
    connect(api, &Api::controlTelemetrySignalErrorFull, this, [this](const Worker *worker) {
        finish(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    openapi::TelemetryControl update;
    update.setCommand(command);
    api->controlTelemetry(update);
    emit changed();
}

void Diagnostics::accept(const Worker &worker, const Snapshot &data, const QString &message,
                         bool control) {
    if (worker.getHttpResponseCode() != 200 || !data.isValid()) {
        finish(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    snapshot_ = data;
    expiry_.stop();
    if (debugActive()) {
        const auto remaining = health().getDebugUntil() - QDateTime::currentSecsSinceEpoch();
        expiry_.start(static_cast<int>(std::clamp(remaining, qint64{1}, qint64{300})) * 1000);
    }
    emit recordsChanged();
    finish(message);
    if (control) {
        emit controlCompleted();
    }
}

void Diagnostics::finish(const QString &message) {
    request_.cancel();
    busy_ = false;
    feedback_ = message;
    emit changed();
}

} // namespace ctxfab
