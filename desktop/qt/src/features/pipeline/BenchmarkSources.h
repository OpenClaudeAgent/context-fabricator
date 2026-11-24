#pragma once

#include "BenchmarkCatalog.h"
#include "BenchmarkCorpus.h"
#include "GitHubActions.h"
#include "GitHubConfiguredRepository.h"
#include "LocalApiRequest.h"

#include <QMap>
#include <QObject>
#include <QQueue>
#include <QSet>
#include <QVariantList>
#include <QtQml/qqmlregistration.h>

class QSettings;

namespace ctxfab {

class BackendSupervisor;

struct BenchmarkCorpusRow {
    Q_GADGET
    QML_VALUE_TYPE(benchmarkCorpusRow)

    Q_PROPERTY(QString key MEMBER key CONSTANT FINAL)
    Q_PROPERTY(QString repository MEMBER repository CONSTANT FINAL)
    Q_PROPERTY(QString revision MEMBER revision CONSTANT FINAL)
    Q_PROPERTY(bool saved MEMBER saved CONSTANT FINAL)
    Q_PROPERTY(bool active MEMBER active CONSTANT FINAL)
    Q_PROPERTY(bool hasActions MEMBER hasActions CONSTANT FINAL)
    Q_PROPERTY(qsizetype runCount MEMBER runCount CONSTANT FINAL)

  public:
    QString key;
    QString repository;
    QString revision;
    bool saved = false;
    bool active = false;
    bool hasActions = false;
    qsizetype runCount = 0;
};

struct BenchmarkWorkflowRow {
    Q_GADGET
    QML_VALUE_TYPE(benchmarkWorkflowRow)

    Q_PROPERTY(QString key MEMBER key CONSTANT FINAL)
    Q_PROPERTY(QString title MEMBER title CONSTANT FINAL)
    Q_PROPERTY(QString subtitle MEMBER subtitle CONSTANT FINAL)
    Q_PROPERTY(bool active MEMBER active CONSTANT FINAL)
    Q_PROPERTY(bool downloaded MEMBER downloaded CONSTANT FINAL)

  public:
    QString key;
    QString title;
    QString subtitle;
    bool active = false;
    bool downloaded = false;
};

class BenchmarkSources : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Provided by the pipeline inspector")

    Q_PROPERTY(QList<ctxfab::BenchmarkCorpusRow> corpora READ corpora NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::BenchmarkWorkflowRow> workflows READ workflows NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::BenchmarkWorkflowRow> connectedWorkflows READ connectedWorkflows NOTIFY
                   changed FINAL)
    Q_PROPERTY(bool canActivateCorpora READ canActivateCorpora NOTIFY changed FINAL)
    Q_PROPERTY(QString pendingCorpus READ pendingCorpus NOTIFY changed FINAL)
    Q_PROPERTY(bool loadingOverview READ loadingOverview NOTIFY changed FINAL)
    Q_PROPERTY(bool connectedLoaded READ connectedLoaded NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool loadedCaptures READ loadedCaptures NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)

  public:
    explicit BenchmarkSources(BackendSupervisor &backend, QSettings &settings);

    [[nodiscard]] QList<ctxfab::BenchmarkCorpusRow> corpora() const;
    [[nodiscard]] QList<ctxfab::BenchmarkWorkflowRow> workflows() const;
    [[nodiscard]] QList<BenchmarkWorkflowRow> connectedWorkflows() const;
    [[nodiscard]] bool canActivateCorpora() const;
    [[nodiscard]] QString pendingCorpus() const { return pendingCorpus_; }
    [[nodiscard]] bool loadingOverview() const { return loadingOverview_; }
    [[nodiscard]] bool connectedLoaded() const { return connectedLoaded_; }
    [[nodiscard]] bool saved(const QString &id) const { return saved_.contains(id); }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] bool loadedCaptures() const { return loadedCaptures_; }
    [[nodiscard]] QString feedback() const { return feedback_; }
    [[nodiscard]] QList<BenchmarkSuite> suites() const;
    [[nodiscard]] bool active(const openapi::BenchmarkReference &reference) const;
    void addCorpus(BenchmarkSuite suite);

    Q_INVOKABLE void open();
    Q_INVOKABLE void activateAllCorpora();
    Q_INVOKABLE void setActive(const QString &id, bool active);
    Q_INVOKABLE void downloadActions(const QString &id);
    Q_INVOKABLE void downloadAllActions();
    Q_INVOKABLE void loadRecordedCaptures();
    Q_INVOKABLE void loadConnectedSources();
    Q_INVOKABLE void setWorkflowActive(const QString &id, bool active);

  signals:
    void changed();
    void suitesChanged();

  private:
    [[nodiscard]] const BenchmarkSuite *corpus(const QString &id) const;
    [[nodiscard]] openapi::DefaultApi *begin();
    void refresh();
    void readNextConnected();
    [[nodiscard]] BenchmarkSuite recordedSuite() const;
    [[nodiscard]] QList<BenchmarkSuite> connectedSuites() const;
    void refreshDownloads();
    void fetchCaptures();
    void downloadNext();
    void activateNext();
    void finish(const QString &message = {});
    void failure(const openapi::HttpRequestWorker *worker);
    void forgetSession();
    [[nodiscard]] bool workflowActive(const openapi::BenchmarkAction &action) const;
    [[nodiscard]] QString workflowKey(const openapi::BenchmarkAction &action) const;
    [[nodiscard]] QList<BenchmarkWorkflowRow>
    workflowRows(const QList<openapi::GitHubActions> &snapshots, bool downloaded) const;

    BackendSupervisor &backend_;
    QSettings &settings_;
    BenchmarkCatalog catalog_ = BenchmarkCatalog::bundled();
    LocalApiRequest request_{120000};
    QMap<QString, openapi::BenchmarkCorpus> saved_;
    QMap<QString, openapi::BenchmarkActionDownload> downloads_;
    QList<openapi::GitHubActions> captures_;
    QList<openapi::GitHubConfiguredRepository> repositories_;
    QList<openapi::GitHubActions> connected_;
    QQueue<qint64> pendingConnected_;
    QSet<QString> inactive_;
    QQueue<openapi::BenchmarkReference> downloadQueue_;
    QQueue<openapi::BenchmarkReference> activationQueue_;
    QStringList activationFailures_;
    QStringList downloadFailures_;
    QString feedback_;
    QString pendingCorpus_;
    bool connectedLoaded_ = false;
    bool loadingOverview_ = false;
    bool opened_ = false;
    bool busy_ = false;
    bool loadedCaptures_ = false;
};

} // namespace ctxfab
