#pragma once

#include "InferenceSetupPlan.h"
#include "InferenceSetupStatus.h"
#include "LocalApiRequest.h"

#include <QTimer>
#include <QUrl>
#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

class BackendSupervisor;

class InferenceSetup final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by the desktop application")
    Q_PROPERTY(bool statusKnown READ statusKnown NOTIFY changed FINAL)
    Q_PROPERTY(bool hasStatus READ hasStatus NOTIFY changed FINAL)
    Q_PROPERTY(bool inProgress READ inProgress NOTIFY changed FINAL)
    Q_PROPERTY(bool waitingForArchive READ waitingForArchive NOTIFY changed FINAL)
    Q_PROPERTY(bool canManageDownloads READ canManageDownloads NOTIFY changed FINAL)
    Q_PROPERTY(bool transferring READ transferring NOTIFY changed FINAL)
    Q_PROPERTY(bool reviewing READ reviewing NOTIFY changed FINAL)
    Q_PROPERTY(bool writing READ writing NOTIFY changed FINAL)
    Q_PROPERTY(bool hasPlan READ hasPlan NOTIFY changed FINAL)
    Q_PROPERTY(QString operationId READ operationId NOTIFY changed FINAL)
    Q_PROPERTY(QString statusText READ statusText NOTIFY changed FINAL)
    Q_PROPERTY(QString statusError READ statusError NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString activeProfile READ activeProfile NOTIFY changed FINAL)
    Q_PROPERTY(QString lastAnswerProfile READ lastAnswerProfile NOTIFY changed FINAL)
    Q_PROPERTY(QString lastEmbeddingProfile READ lastEmbeddingProfile NOTIFY changed FINAL)
    Q_PROPERTY(bool lastEmbedding READ lastEmbedding NOTIFY changed FINAL)
    Q_PROPERTY(QString planAnswerProfile READ planAnswerProfile NOTIFY changed FINAL)
    Q_PROPERTY(QString planEmbeddingProfile READ planEmbeddingProfile NOTIFY changed FINAL)
    Q_PROPERTY(bool planAnswerInstalled READ planAnswerInstalled NOTIFY changed FINAL)
    Q_PROPERTY(bool planEmbeddingInstalled READ planEmbeddingInstalled NOTIFY changed FINAL)
    Q_PROPERTY(bool planRuntimeRequired READ planRuntimeRequired NOTIFY changed FINAL)
    Q_PROPERTY(bool planNeedsArchive READ planNeedsArchive NOTIFY changed FINAL)
    Q_PROPERTY(bool planHasTransfer READ planHasTransfer NOTIFY changed FINAL)
    Q_PROPERTY(bool planAllowed READ planAllowed NOTIFY changed FINAL)
    Q_PROPERTY(QString planTransfer READ planTransfer NOTIFY changed FINAL)
    Q_PROPERTY(QString planPeak READ planPeak NOTIFY changed FINAL)
    Q_PROPERTY(QString planAvailable READ planAvailable NOTIFY changed FINAL)
    Q_PROPERTY(QString planBlocker READ planBlocker NOTIFY changed FINAL)

  public:
    explicit InferenceSetup(BackendSupervisor &backend);

    bool statusKnown() const { return statusKnown_; }
    bool hasStatus() const { return status_.isValid(); }
    bool inProgress() const;
    bool waitingForArchive() const;
    bool canManageDownloads() const;
    bool transferring() const;
    bool answerReady() const { return status_.isAnswerReady(); }
    bool reviewing() const { return requestState_ == Request::Review; }
    bool writing() const;
    bool hasPlan() const { return plan_.isValid(); }
    QString operationId() const { return status_.getOperationId(); }
    QString statusText() const;
    QString statusError() const { return status_.getError(); }
    QString feedback() const;
    QString activeProfile() const;
    QString lastAnswerProfile() const { return status_.getSelection().getAnswerProfile(); }
    QString lastEmbeddingProfile() const { return status_.getSelection().getEmbeddingProfile(); }
    bool lastEmbedding() const { return status_.getSelection().isEmbedding(); }
    QString planAnswerProfile() const { return plan_.getSelection().getAnswerProfile(); }
    QString planEmbeddingProfile() const { return plan_.getSelection().getEmbeddingProfile(); }
    bool planAnswerInstalled() const { return plan_.isAnswerInstalled(); }
    bool planEmbeddingInstalled() const { return plan_.isEmbeddingInstalled(); }
    bool planRuntimeRequired() const { return plan_.isRuntimeRequired(); }
    bool planNeedsArchive() const;
    bool planHasTransfer() const { return plan_.getMaximumTransferBytes() > 0; }
    bool planAllowed() const;
    QString planTransfer() const;
    QString planPeak() const;
    QString planAvailable() const;
    QString planBlocker() const;
    bool canConfirm(const QUrl &archive) const;

    Q_INVOKABLE void setObserved(bool observed);
    Q_INVOKABLE void review(const QString &answer, bool embedding, const QString &embeddingProfile,
                            bool useLocalArchive);
    Q_INVOKABLE void discardPlan();
    Q_INVOKABLE void confirm(const QUrl &archive);
    Q_INVOKABLE void provideArchive(const QString &operation, const QUrl &archive);
    Q_INVOKABLE void cancelArchive(const QString &operation);

  signals:
    void changed();
    void modelsChanged();

  private:
    enum class Request { Idle, Status, Review, Confirm, ProvideArchive, CancelArchive };
    void reset();
    QString outcomeText() const;
    bool canReview() const;
    void updateObservation();
    void pollStatus();
    openapi::DefaultApi *begin(Request state);
    void receivePlan(const openapi::HttpRequestWorker &worker,
                     const openapi::InferenceSetupPlan &plan,
                     const openapi::InferenceSetupSelection &requested);
    void receiveStatus(const openapi::HttpRequestWorker &worker,
                       const openapi::InferenceSetupStatus &status);
    void publishStatus(const openapi::InferenceSetupStatus &status);
    void reject(const openapi::HttpRequestWorker &worker);
    void finish();
    bool matchesWaitingOperation(const QString &operation) const;
    void watchWrite(openapi::DefaultApi &api);

    BackendSupervisor &backend_;
    QTimer poll_;
    LocalApiRequest request_;
    Request requestState_ = Request::Idle;
    openapi::InferenceSetupPlan plan_;
    openapi::InferenceSetupStatus status_;
    QString expectedOperation_;
    QString statusFeedback_;
    QString commandFeedback_;
    bool observed_ = false;
    bool statusKnown_ = false;
};

} // namespace ctxfab
