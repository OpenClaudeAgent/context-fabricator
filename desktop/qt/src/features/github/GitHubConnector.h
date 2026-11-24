#pragma once

#include "GitHubCollection.h"
#include "GitHubCollectionTraceLink.h"
#include "GitHubRepositoryList.h"
#include "GitHubSelection.h"
#include "GitHubState.h"
#include "GitHubUsage.h"
#include "LocalApiRequest.h"

#include <QNetworkReply>
#include <QObject>
#include <QStringList>
#include <QTimer>
#include <QUrl>
#include <QtQml/qqmlregistration.h>

#include <optional>

namespace ctxfab {

class BackendSupervisor;

class GitHubConnector final : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(GitHubConnector)
    QML_UNCREATABLE("Created by the desktop application")

    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(Credential credential READ credential NOTIFY changed FINAL)
    Q_PROPERTY(QString login READ login NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::GitHubRepositoryList *repositoryList READ repositoryList CONSTANT FINAL)
    Q_PROPERTY(ctxfab::GitHubRepositoryList *candidateList READ candidateList CONSTANT FINAL)
    Q_PROPERTY(bool moreCandidates READ moreCandidates NOTIFY changed FINAL)
    Q_PROPERTY(QStringList branches READ branches NOTIFY changed FINAL)
    Q_PROPERTY(QStringList matchingBranches READ matchingBranches NOTIFY changed FINAL)
    Q_PROPERTY(bool branchesLoaded READ branchesLoaded NOTIFY changed FINAL)
    Q_PROPERTY(bool moreBranches READ moreBranches NOTIFY changed FINAL)
    Q_PROPERTY(bool draftOpen READ draftOpen NOTIFY changed FINAL)
    Q_PROPERTY(bool editing READ editing NOTIFY changed FINAL)
    Q_PROPERTY(bool draftNeedsReview READ draftNeedsReview NOTIFY changed FINAL)
    Q_PROPERTY(bool draftCanReview READ draftCanReview NOTIFY changed FINAL)
    Q_PROPERTY(QString draftRepository READ draftRepository NOTIFY changed FINAL)
    Q_PROPERTY(QString draftBranch READ draftBranch NOTIFY changed FINAL)
    Q_PROPERTY(int draftInterval READ draftInterval NOTIFY changed FINAL)
    Q_PROPERTY(int draftFeatures READ draftFeatures NOTIFY changed FINAL)
    Q_PROPERTY(bool draftVerified READ draftVerified NOTIFY changed FINAL)
    Q_PROPERTY(int minimumInterval READ minimumInterval NOTIFY changed FINAL)
    Q_PROPERTY(int maximumInterval READ maximumInterval NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::GitHubCollection draftCollection READ draftCollection NOTIFY changed FINAL)
    Q_PROPERTY(QString collectionFeedback READ collectionFeedback NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::GitHubUsage usage READ usage NOTIFY changed FINAL)
    Q_PROPERTY(bool draftDataKnown READ draftDataKnown NOTIFY changed FINAL)
    Q_PROPERTY(bool draftDataPaused READ draftDataPaused NOTIFY changed FINAL)
    Q_PROPERTY(QString draftDataFeedback READ draftDataFeedback NOTIFY changed FINAL)
    Q_PROPERTY(bool canRefreshBranch READ canRefreshBranch NOTIFY changed FINAL)
    Q_PROPERTY(bool hasCommitHistory READ hasCommitHistory NOTIFY changed FINAL)
    Q_PROPERTY(bool canRefreshHistory READ canRefreshHistory NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::GitHubCollectionTraceLink> historyTraces READ historyTraces NOTIFY
                   changed FINAL)
    Q_PROPERTY(QList<ctxfab::GitHubCollectionTraceLink> actionsTraces READ actionsTraces NOTIFY
                   changed FINAL)
    Q_PROPERTY(bool canBrowseGraph READ canBrowseGraph NOTIFY changed FINAL)

  public:
    enum Credential { Unknown, Disconnected, Connected, ReconnectRequired };
    Q_ENUM(Credential)
    enum Feature { BranchFiles = 1, CommitHistory = 2, Actions = 4, CodeGraph = 8 };
    Q_ENUM(Feature)

    explicit GitHubConnector(BackendSupervisor &backend);

    [[nodiscard]] bool ready() const { return state_.has_value(); }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] Credential credential() const;
    [[nodiscard]] QString login() const;
    [[nodiscard]] QString feedback() const;
    GitHubRepositoryList *repositoryList() { return &repositoryList_; }
    GitHubRepositoryList *candidateList() { return &candidateList_; }
    void retranslate();
    [[nodiscard]] bool moreCandidates() const { return candidatePage_ > 0; }
    [[nodiscard]] QStringList branches() const { return branches_; }
    [[nodiscard]] QStringList matchingBranches() const;
    [[nodiscard]] bool branchesLoaded() const { return branchesLoaded_; }
    [[nodiscard]] bool moreBranches() const { return branchPage_ > 0; }
    [[nodiscard]] bool draftOpen() const { return draft_.has_value(); }
    [[nodiscard]] bool editing() const { return draft_ && draft_->getExpectedId() > 0; }
    [[nodiscard]] bool draftNeedsReview() const { return draftOpen() && review_; }
    [[nodiscard]] bool draftCanReview() const;
    [[nodiscard]] QString draftRepository() const;
    [[nodiscard]] QString draftBranch() const;
    [[nodiscard]] int draftInterval() const;
    [[nodiscard]] int draftFeatures() const;
    [[nodiscard]] bool draftVerified() const;
    [[nodiscard]] int minimumInterval() const;
    [[nodiscard]] int maximumInterval() const;
    [[nodiscard]] QUrl tokenUrl(bool actions) const;
    [[nodiscard]] GitHubCollection draftCollection() const;
    [[nodiscard]] QString collectionFeedback() const;
    [[nodiscard]] GitHubUsage usage() const;
    [[nodiscard]] bool draftDataKnown() const { return draftDataPaused_.has_value(); }
    [[nodiscard]] bool draftDataPaused() const { return draftDataPaused_.value_or(false); }
    [[nodiscard]] QString draftDataFeedback() const;
    [[nodiscard]] bool canRefreshBranch() const;
    [[nodiscard]] bool hasCommitHistory() const;
    [[nodiscard]] bool canRefreshHistory() const;
    [[nodiscard]] QList<GitHubCollectionTraceLink> historyTraces() const;
    [[nodiscard]] QList<GitHubCollectionTraceLink> actionsTraces() const;
    [[nodiscard]] bool canBrowseGraph() const;
    [[nodiscard]] std::optional<openapi::GitHubConfiguredRepository> configuredDraft() const;

    Q_INVOKABLE void reload();
    Q_INVOKABLE void openTokenPage(bool actions);
    Q_INVOKABLE void authorize(const QString &token);
    Q_INVOKABLE void disconnectAccount();
    Q_INVOKABLE void discover();
    Q_INVOKABLE void loadMoreCandidates();
    Q_INVOKABLE void filterCandidates(const QString &filter);
    Q_INVOKABLE void addRepository(const QString &repository);
    Q_INVOKABLE void chooseCandidate(int row);
    Q_INVOKABLE void editRepository(int row);
    Q_INVOKABLE void loadMoreBranches();
    Q_INVOKABLE void filterBranches(const QString &filter);
    Q_INVOKABLE void reviewDraft(const QString &repository, const QString &branch, int interval,
                                 int features);
    Q_INVOKABLE void closeDraft();
    Q_INVOKABLE void saveRepository(const QString &repository, const QString &branch, int interval,
                                    int features);
    Q_INVOKABLE void verifyAccess();
    Q_INVOKABLE void removeRepository();
    Q_INVOKABLE void setCollectionVisible(bool visible);
    Q_INVOKABLE void refreshBranch();
    Q_INVOKABLE void refreshHistory();
    Q_INVOKABLE void browseGraph();
    Q_INVOKABLE void refreshGraphSource(const QString &branch);
    Q_INVOKABLE void loadStorage();
    Q_INVOKABLE void setStorageLimit(int limit);
    Q_INVOKABLE void setRequestLimit(int limit);
    Q_INVOKABLE void loadDraftData();
    Q_INVOKABLE void purgeRepositoryData();

  signals:
    void changed();
    void repositoryPurged(qint64 id);
    void graphRequested(qint64 id, const QString &repository, const QString &branch);

  private:
    using Worker = openapi::HttpRequestWorker;
    using Api = openapi::DefaultApi;
    using ErrorSignal = void (Api::*)(Worker *, QNetworkReply::NetworkError, const QString &);
    using StateSignal = void (Api::*)(Worker *, openapi::GitHubState);
    enum class DraftResult { Preserve, Close, Verify };
    enum class ConflictHandling { ReviewDraft, ReportProblem };

    template <typename Payload, typename Handler>
    Api *begin(void (Api::*response)(Worker *, Payload), ErrorSignal error, Handler handler,
               ConflictHandling conflict = ConflictHandling::ReviewDraft);
    Api *beginState(StateSignal response, ErrorSignal error,
                    DraftResult draft = DraftResult::Preserve,
                    ConflictHandling conflict = ConflictHandling::ReviewDraft);
    Api *beginCommand(ErrorSignal error, ConflictHandling conflict);
    void backendChanged();
    Api *startCommand(std::optional<int> timeoutMs = {});
    void acceptState(const openapi::GitHubState &state, DraftResult draft,
                     const QString &feedback = {});
    void readCollectionState(Api &api, const QString &feedback = {});
    [[nodiscard]] bool validState(const openapi::GitHubState &state) const;
    void updateState(const openapi::GitHubState &state, DraftResult draft);
    void scheduleCollectionPoll();
    void stopCollectionPoll();
    void pollCollection();
    void finishCollectionPoll(const QString &feedback = {});
    void rejected(const Worker &worker, ConflictHandling conflict);
    void finish(const QString &feedback = {});
    void clearDiscovery();
    void updateRepositoryList();
    void buildCandidateRows();
    void loadCandidates(int page);
    void createDraft(const QString &repository, const QString &branch, qint64 id);
    void loadBranches(int page);
    void clearBranches();
    void clearDraftData();
    void finishDraftData(const QString &feedback = {});
    bool updateStorage(const openapi::GitHubStorageBudget &budget);
    void acceptStorage(const openapi::GitHubStorageBudget &budget);
    void acceptPurge(Api &api, qint64 id, const Worker &worker,
                     const openapi::GitHubStorageBudget &budget);
    void clearPurgedCollection(qint64 id);
    [[nodiscard]] std::optional<openapi::GitHubConfiguredRepository> configured(qint64 id) const;
    [[nodiscard]] std::optional<openapi::GitHubConfiguredRepository> graphRepository() const;

    BackendSupervisor &backend_;
    LocalApiRequest request_{15000};
    LocalApiRequest draftDataRequest_;
    QTimer collectionTimer_;
    std::optional<openapi::GitHubState> state_;
    std::optional<openapi::GitHubSelection> draft_;
    std::optional<openapi::GitHubStorageBudget> storage_;
    std::optional<bool> draftDataPaused_;
    QList<openapi::GitHubRepository> discovered_;
    GitHubRepositoryList repositoryList_;
    GitHubRepositoryList candidateList_;
    QStringList branches_;
    QString branchFilter_;
    QString filter_;
    QString feedback_;
    QString collectionFeedback_;
    QString draftDataFeedback_;
    int candidatePage_ = 0;
    int branchPage_ = 0;
    bool busy_ = false;
    bool review_ = false;
    bool branchesLoaded_ = false;
    bool collectionVisible_ = false;
    bool polling_ = false;
};

} // namespace ctxfab
