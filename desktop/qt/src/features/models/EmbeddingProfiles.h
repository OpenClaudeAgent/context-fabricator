#pragma once

#include "EmbeddingModelSelection.h"
#include "LocalApiRequest.h"

#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

class BackendSupervisor;

class EmbeddingProfiles final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool loaded READ loaded NOTIFY changed FINAL)
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(QStringList labels READ labels NOTIFY changed FINAL)
    Q_PROPERTY(int selectedIndex READ selectedIndex NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)

  public:
    explicit EmbeddingProfiles(BackendSupervisor &backend);
    bool loaded() const { return snapshot_.isValid(); }
    bool ready() const;
    bool busy() const { return busy_; }
    QStringList labels() const;
    int selectedIndex() const;
    QString feedback() const { return feedback_; }

    Q_INVOKABLE void setObserved(bool observed);
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void select(int index);

  signals:
    void changed();
    void selected();

  private:
    void reset();
    void send(const QString &profile);
    void receive(const openapi::HttpRequestWorker &worker,
                 const openapi::EmbeddingModelSelection &snapshot);
    void reject(const openapi::HttpRequestWorker &worker);
    void finish();

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    openapi::EmbeddingModelSelection snapshot_;
    QString requestedProfile_;
    QString feedback_;
    bool observed_ = false;
    bool busy_ = false;
    bool refreshQueued_ = false;
};

} // namespace ctxfab
