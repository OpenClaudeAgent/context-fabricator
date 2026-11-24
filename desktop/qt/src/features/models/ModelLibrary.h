#pragma once

#include "InferenceStoragePlan.h"
#include "LibraryEntries.h"
#include "LocalApiRequest.h"
#include "ModelRemovalPlan.h"

#include <QTimer>

namespace ctxfab {

class BackendSupervisor;

class ModelLibrary final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")

    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool catalogReady READ catalogReady NOTIFY changed FINAL)
    Q_PROPERTY(bool empty READ empty NOTIFY changed FINAL)
    Q_PROPERTY(bool transferActive READ transferActive NOTIFY changed FINAL)
    Q_PROPERTY(QString query READ query WRITE setQuery NOTIFY queryChanged FINAL)
    Q_PROPERTY(QString catalogFeedback READ catalogFeedback NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString pendingProfile READ pendingProfile NOTIFY changed FINAL)
    Q_PROPERTY(QString feedbackProfile READ feedbackProfile NOTIFY changed FINAL)
    Q_PROPERTY(QString removalMessage READ removalMessage NOTIFY changed FINAL)
    Q_PROPERTY(QString storageSummary READ storageSummary NOTIFY changed FINAL)
    Q_PROPERTY(QString storageDetails READ storageDetails NOTIFY changed FINAL)
    Q_PROPERTY(QString storageFeedback READ storageFeedback NOTIFY changed FINAL)
    Q_PROPERTY(QString runtimeImportEstimate READ runtimeImportEstimate NOTIFY changed FINAL)
    Q_PROPERTY(QString runtimeDownloadEstimate READ runtimeDownloadEstimate NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::LibraryEntries *downloads READ downloads CONSTANT FINAL)
    Q_PROPERTY(ctxfab::LibraryEntries *answerModels READ answerModels CONSTANT FINAL)
    Q_PROPERTY(ctxfab::LibraryEntries *embeddingModels READ embeddingModels CONSTANT FINAL)
    Q_PROPERTY(ctxfab::LibraryEntries *otherModels READ otherModels CONSTANT FINAL)
    Q_PROPERTY(ctxfab::LibraryEntries *setupAnswers READ setupAnswers CONSTANT FINAL)
    Q_PROPERTY(ctxfab::LibraryEntries *setupEmbeddings READ setupEmbeddings CONSTANT FINAL)

  public:
    explicit ModelLibrary(BackendSupervisor &backend);

    bool ready() const;
    bool catalogReady() const { return catalogReady_; }
    bool empty() const { return catalog_.isEmpty(); }
    bool transferActive() const;
    QString query() const { return query_; }
    QString catalogFeedback() const;
    QString feedback() const;
    QString feedbackProfile() const { return feedbackProfile_; }
    QString pendingProfile() const { return pendingProfile_; }
    QString removalMessage() const;
    QString storageSummary() const;
    QString storageDetails() const;
    QString storageFeedback() const;
    QString runtimeImportEstimate() const;
    QString runtimeDownloadEstimate() const;
    LibraryEntries *downloads() { return &downloads_; }
    LibraryEntries *answerModels() { return &answerModels_; }
    LibraryEntries *embeddingModels() { return &embeddingModels_; }
    LibraryEntries *otherModels() { return &otherModels_; }
    LibraryEntries *setupAnswers() { return &setupAnswers_; }
    LibraryEntries *setupEmbeddings() { return &setupEmbeddings_; }

    void setQuery(const QString &query);
    void retranslate();
    Q_INVOKABLE void open();
    Q_INVOKABLE void close();
    Q_INVOKABLE void refresh();
    Q_INVOKABLE LibraryModel modelForProfile(const QString &profile) const {
        return entry(profile);
    }
    Q_INVOKABLE void act(const ctxfab::LibraryModel &selected);
    Q_INVOKABLE void discard(const QString &profile);
    Q_INVOKABLE void cancelRemoval();
    Q_INVOKABLE void confirmRemoval();
    Q_INVOKABLE QUrl modelFolder() const;

  signals:
    void changed();
    void queryChanged();
    void installedModelsChanged();

  private:
    enum class CommandState { Idle, Writing, AwaitingCatalog, PreparingRemoval };
    void resetSession();
    void cancelReads();
    void rebuildEntries();
    void rebuildSetupChoices();
    LibraryModel entry(const QString &profile) const;
    QStringList installedProfiles() const;
    bool canIssueCommand() const;
    openapi::DefaultApi *beginCommand(const QString &profile, CommandState state);
    void sendDownload(openapi::DefaultApi &api, const QString &profile);
    void sendTransferControl(openapi::DefaultApi &api, const QString &profile,
                             LibraryModel::Action action);
    void sendRemovalPlan(openapi::DefaultApi &api, const QString &profile);
    void finishRemovalPlan(const openapi::HttpRequestWorker &worker,
                           const openapi::ModelRemovalPlan &plan);
    void commandAccepted(const openapi::HttpRequestWorker &worker, int expectedStatus);
    void commandRejected(const openapi::HttpRequestWorker &worker);
    void removalRejected(const openapi::HttpRequestWorker &worker);
    void endCommand();
    void loadCatalog();
    void receiveCatalog(const openapi::HttpRequestWorker &worker,
                        const QList<openapi::ModelDownload> &models);
    void loadStorage();
    void receiveStorage(const openapi::HttpRequestWorker &worker,
                        const openapi::InferenceStoragePlan &plan);
    QString runtimeEstimate(const openapi::InstallStoragePlan &plan, const char *label) const;

    BackendSupervisor &backend_;
    QTimer poll_;
    LocalApiRequest catalogRequest_;
    LocalApiRequest storageRequest_;
    LocalApiRequest commandRequest_;
    QList<openapi::ModelDownload> catalog_;
    openapi::InferenceStoragePlan storage_;
    openapi::ModelRemovalPlan removal_;
    LibraryEntries downloads_;
    LibraryEntries answerModels_;
    LibraryEntries embeddingModels_;
    LibraryEntries otherModels_;
    LibraryEntries setupAnswers_;
    LibraryEntries setupEmbeddings_;
    QString query_;
    QString pendingProfile_;
    QString feedbackProfile_;
    QString commandFeedback_;
    CommandState command_ = CommandState::Idle;
    bool catalogReady_ = false;
    bool catalogBusy_ = false;
    bool storageBusy_ = false;
    bool storageFailed_ = false;
    bool catalogFailed_ = false;
    bool staleRemoval_ = false;
};

} // namespace ctxfab
