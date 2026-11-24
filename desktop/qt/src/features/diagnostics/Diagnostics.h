#pragma once

#include "LocalApiRequest.h"
#include "TelemetrySnapshot.h"
#include "TraceRecord.h"

#include <QObject>
#include <QTimer>
#include <QUrl>
#include <QtQmlIntegration/qqmlintegration.h>

#include <optional>

namespace ctxfab {

class BackendSupervisor;

class Diagnostics final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString healthStatus READ healthStatus NOTIFY changed FINAL)
    Q_PROPERTY(bool recording READ recording NOTIFY changed FINAL)
    Q_PROPERTY(bool debugActive READ debugActive NOTIFY changed FINAL)
    Q_PROPERTY(QString debugExpiry READ debugExpiry NOTIFY changed FINAL)
    Q_PROPERTY(QString storageUsed READ storageUsed NOTIFY changed FINAL)
    Q_PROPERTY(QString storageBudget READ storageBudget NOTIFY changed FINAL)
    Q_PROPERTY(double storageFraction READ storageFraction NOTIFY changed FINAL)
    Q_PROPERTY(QString storageRetention READ storageRetention NOTIFY changed FINAL)
    Q_PROPERTY(QString storageOldest READ storageOldest NOTIFY changed FINAL)
    Q_PROPERTY(QString queuedCount READ queuedCount NOTIFY changed FINAL)
    Q_PROPERTY(QString droppedCount READ droppedCount NOTIFY changed FINAL)
    Q_PROPERTY(QString purgedCount READ purgedCount NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::TraceRecord> traces READ traces NOTIFY recordsChanged FINAL)
    Q_PROPERTY(QList<ctxfab::TraceRecord> metrics READ metrics NOTIFY recordsChanged FINAL)

  public:
    explicit Diagnostics(BackendSupervisor &backend);

    [[nodiscard]] bool ready() const { return snapshot_.has_value(); }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] QString healthStatus() const;
    [[nodiscard]] bool recording() const;
    [[nodiscard]] bool debugActive() const;
    [[nodiscard]] QString debugExpiry() const;
    [[nodiscard]] QString storageUsed() const;
    [[nodiscard]] QString storageBudget() const;
    [[nodiscard]] double storageFraction() const;
    [[nodiscard]] QString storageRetention() const;
    [[nodiscard]] QString storageOldest() const;
    [[nodiscard]] QString queuedCount() const;
    [[nodiscard]] QString droppedCount() const;
    [[nodiscard]] QString purgedCount() const;
    [[nodiscard]] QList<TraceRecord> traces() const;
    [[nodiscard]] QList<TraceRecord> metrics() const;

    Q_INVOKABLE void setPageVisible(bool visible);
    Q_INVOKABLE void setFilter(const QString &query);
    Q_INVOKABLE void setIncludeBackgroundChecks(bool include);
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void toggleDebug();
    Q_INVOKABLE void purgeExpired();
    Q_INVOKABLE void saveSnapshot(const QUrl &destination);

  signals:
    void changed();
    void recordsChanged();
    void controlCompleted();

  private:
    openapi::DefaultApi *begin();
    void control(const QString &command, const QString &message);
    void accept(const openapi::HttpRequestWorker &worker, const openapi::TelemetrySnapshot &data,
                const QString &message, bool control = false);
    void finish(const QString &message);
    void backendChanged();
    [[nodiscard]] openapi::TelemetryHealth health() const;

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    QTimer expiry_;
    std::optional<openapi::TelemetrySnapshot> snapshot_;
    QString feedback_;
    QString filter_;
    bool visible_ = false;
    bool busy_ = false;
    bool includeBackgroundChecks_ = false;
};

} // namespace ctxfab
