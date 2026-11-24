#pragma once

#include "LocalApiRequest.h"
#include <QObject>
#include <QtQmlIntegration/qqmlintegration.h>

#include <optional>
#include <utility>

namespace ctxfab {

class BackendSupervisor;
namespace openapi {
class Status;
} // namespace openapi

class Overview final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool managed READ managed CONSTANT FINAL)
    Q_PROPERTY(bool canRestart READ canRestart NOTIFY changed FINAL)
    Q_PROPERTY(QString serviceState READ serviceState NOTIFY changed FINAL)
    Q_PROPERTY(int indexedDocuments READ indexedDocuments NOTIFY changed FINAL)
    Q_PROPERTY(QString collectedDocuments READ collectedDocuments NOTIFY changed FINAL)
    Q_PROPERTY(int connectedSources READ connectedSources NOTIFY changed FINAL)
    Q_PROPERTY(int outboundAttempts READ outboundAttempts NOTIFY changed FINAL)
    Q_PROPERTY(QString pendingOutcomes READ pendingOutcomes NOTIFY changed FINAL)
    Q_PROPERTY(QString uncertainOutcomes READ uncertainOutcomes NOTIFY changed FINAL)
    Q_PROPERTY(QString embeddingState READ embeddingState NOTIFY changed FINAL)
    Q_PROPERTY(QString embeddingProgress READ embeddingProgress NOTIFY changed FINAL)

  public:
    explicit Overview(BackendSupervisor &backend);

    bool busy() const { return busy_; }
    bool ready() const { return ready_; }
    bool managed() const;
    bool canRestart() const;
    QString serviceState() const;
    int indexedDocuments() const { return indexedDocuments_; }
    QString collectedDocuments() const { return collectedDocuments_; }
    int connectedSources() const { return connectedSources_; }
    int outboundAttempts() const { return outboundAttempts_; }
    QString pendingOutcomes() const { return pendingOutcomes_; }
    QString uncertainOutcomes() const { return uncertainOutcomes_; }
    QString embeddingState() const;
    QString embeddingProgress() const;

    Q_INVOKABLE void refresh();
    Q_INVOKABLE void restart();

  signals:
    void changed();

  private:
    void backendChanged();
    void acceptStatus(const openapi::Status &status);
    void finish(const char *serviceState, bool ready = false);

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    bool busy_ = false;
    bool ready_ = false;
    const char *serviceState_ = nullptr;
    int indexedDocuments_ = 0;
    QString collectedDocuments_;
    int connectedSources_ = 0;
    int outboundAttempts_ = 0;
    QString pendingOutcomes_;
    QString uncertainOutcomes_;
    const char *embeddingState_ = nullptr;
    std::optional<std::pair<qint64, qint64>> embeddingCounts_;
};

} // namespace ctxfab
