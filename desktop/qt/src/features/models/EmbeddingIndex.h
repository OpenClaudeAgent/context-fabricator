#pragma once

#include "EmbeddingIndexState.h"
#include "LocalApiRequest.h"

#include <QTimer>
#include <QtQmlIntegration/qqmlintegration.h>

#include <optional>

namespace ctxfab {

class BackendSupervisor;

class EmbeddingIndex final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool loaded READ loaded NOTIFY changed FINAL)
    Q_PROPERTY(bool enabled READ enabled NOTIFY changed FINAL)
    Q_PROPERTY(bool writing READ writing NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString removalSummary READ removalSummary NOTIFY changed FINAL)

  public:
    explicit EmbeddingIndex(BackendSupervisor &backend);
    bool loaded() const { return snapshot_.isValid(); }
    bool enabled() const { return snapshot_.isEnabled(); }
    qint64 epoch() const { return snapshot_.getEpoch(); }
    bool loading() const { return pending_ != Request::None; }
    bool writing() const;
    QString feedback() const;
    QString removalSummary() const;

    Q_INVOKABLE void setObserved(bool observed);
    void refresh();
    Q_INVOKABLE void setEnabled(bool enabled);
    Q_INVOKABLE void remove();

  signals:
    void changed();
    void updated();

  private:
    enum class Request { None, Observe, Toggle, Remove };
    struct Removal {
        qint64 vectors;
        qint64 bytes;
    };
    void reset();
    openapi::DefaultApi *begin(Request request);
    void receive(const openapi::HttpRequestWorker &worker,
                 const openapi::EmbeddingIndexState &state, std::optional<Removal> removal = {});
    void reject(const openapi::HttpRequestWorker &worker);
    void finish();
    bool canWrite() const;

    BackendSupervisor &backend_;
    QTimer poll_;
    LocalApiRequest request_;
    openapi::EmbeddingIndexState snapshot_;
    std::optional<Removal> removal_;
    QString readError_;
    QString commandError_;
    Request pending_ = Request::None;
};

} // namespace ctxfab
