#pragma once

#include "AnswerResult.h"
#include "BenchmarkSources.h"
#include "TraceAnswerDetails.h"

#include <QObject>
#include <QUrl>
#include <QVariantList>
#include <QtQml/qqmlregistration.h>

namespace ctxfab::openapi {
class RetrievalProgress;
}

namespace ctxfab {

struct BenchmarkResultRow {
    Q_GADGET
    QML_VALUE_TYPE(benchmarkResultRow)

    Q_PROPERTY(QString query MEMBER query CONSTANT FINAL)
    Q_PROPERTY(QString source MEMBER source CONSTANT FINAL)
    Q_PROPERTY(QUrl reference MEMBER reference CONSTANT FINAL)
    Q_PROPERTY(QString expected MEMBER expected CONSTANT FINAL)
    Q_PROPERTY(QString outcome MEMBER outcome CONSTANT FINAL)
    Q_PROPERTY(QString error MEMBER error CONSTANT FINAL)
    Q_PROPERTY(QString answer MEMBER answer CONSTANT FINAL)
    Q_PROPERTY(bool complete MEMBER complete CONSTANT FINAL)

  public:
    QString query;
    QString source;
    QUrl reference;
    QString expected;
    QString outcome;
    QString error;
    QString answer;
    bool complete = false;
};

class PipelineInspector : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Provided by the workspace")

    Q_PROPERTY(ctxfab::BenchmarkSources *sources READ sources CONSTANT FINAL)
    Q_PROPERTY(QStringList suites READ suites NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::BenchmarkResultRow> rows READ rows NOTIFY changed FINAL)
    Q_PROPERTY(int selectedSuite READ selectedSuite NOTIFY changed FINAL)
    Q_PROPERTY(QString selectedCorpus READ selectedCorpus NOTIFY changed FINAL)
    Q_PROPERTY(bool selectedCorpusSaved READ selectedCorpusSaved NOTIFY changed FINAL)
    Q_PROPERTY(QString completionInput READ completionInput NOTIFY changed FINAL)
    Q_PROPERTY(bool sourceReady READ sourceReady NOTIFY changed FINAL)
    Q_PROPERTY(bool hybridAvailable READ hybridAvailable NOTIFY changed FINAL)
    Q_PROPERTY(bool hybridProblem READ hybridProblem NOTIFY changed FINAL)
    Q_PROPERTY(bool hybrid READ hybrid WRITE setHybrid NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool stopping READ stopping NOTIFY changed FINAL)
    Q_PROPERTY(int finishedCount READ finishedCount NOTIFY changed FINAL)
    Q_PROPERTY(int totalCount READ totalCount NOTIFY changed FINAL)
    Q_PROPERTY(qint64 indexed READ indexed NOTIFY changed FINAL)
    Q_PROPERTY(qint64 indexTotal READ indexTotal NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString inspectedQuestion READ inspectedQuestion NOTIFY changed FINAL)
    Q_PROPERTY(QString traceId READ traceId NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::TraceAnswerDetails details READ details NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::AnswerResult result READ result NOTIFY changed FINAL)

  public:
    PipelineInspector(BackendSupervisor &backend, QSettings &settings);

    [[nodiscard]] BenchmarkSources *sources() { return &sources_; }
    [[nodiscard]] QStringList suites() const;
    [[nodiscard]] QList<ctxfab::BenchmarkResultRow> rows() const;
    [[nodiscard]] int selectedSuite() const { return selectedSuite_; }
    [[nodiscard]] QString selectedCorpus() const;
    [[nodiscard]] bool selectedCorpusSaved() const { return sources_.saved(selectedCorpus()); }
    [[nodiscard]] QString completionInput() const;
    [[nodiscard]] bool sourceReady() const;
    [[nodiscard]] bool hybridAvailable() const;
    [[nodiscard]] bool hybridProblem() const { return hybridProblem_; }
    [[nodiscard]] bool hybrid() const { return hybrid_; }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] bool stopping() const { return stopping_; }
    [[nodiscard]] int finishedCount() const { return finished_; }
    [[nodiscard]] int totalCount() const { return total_; }
    [[nodiscard]] qint64 indexed() const { return indexed_; }
    [[nodiscard]] qint64 indexTotal() const { return indexTotal_; }
    [[nodiscard]] QString feedback() const { return feedback_; }
    [[nodiscard]] QString inspectedQuestion() const { return inspectedQuestion_; }
    [[nodiscard]] QString traceId() const { return traceId_; }
    [[nodiscard]] TraceAnswerDetails details() const {
        return TraceAnswerDetails(result_.payload());
    }
    [[nodiscard]] AnswerResult result() const { return result_; }

    Q_INVOKABLE void open();
    Q_INVOKABLE void selectSuite(int index);
    Q_INVOKABLE void selectDefaultBenchmark();
    Q_INVOKABLE void setHybrid(bool enabled);
    Q_INVOKABLE void runQuestion(int index);
    Q_INVOKABLE void runAll();
    Q_INVOKABLE void ask(const QString &question, int selectedCase);
    Q_INVOKABLE void inspectResult(int index);
    Q_INVOKABLE void stop();
    Q_INVOKABLE void copyCompletionInput();
    Q_INVOKABLE void importFiles(const QList<QUrl> &urls);
    Q_INVOKABLE void importSuite(const QUrl &url);
    Q_INVOKABLE QString stageText(int stage) const;

  signals:
    void changed();

  private:
    struct Pending {
        int row;
        BenchmarkCase question;
    };
    struct Result {
        AnswerResult result = {};
        QString error = {};
        QString trace = {};
    };

    [[nodiscard]] const BenchmarkSuite *selected() const;
    void rebuildSuites();
    void importTemporary(QList<openapi::BenchmarkDocument> documents,
                         QList<BenchmarkCase> questions);
    void askInSuite(const BenchmarkSuite &suite, const QString &query, int selectedCase);
    void begin(BenchmarkSuite scope, QList<Pending> cases);
    void indexBatch();
    void acceptIndexProgress(const openapi::RetrievalProgress &progress);
    void rejectIndexBatch(const openapi::HttpRequestWorker *worker);
    void sendNext();
    void sendBenchmark(openapi::DefaultApi &api, const BenchmarkCase &question);
    void setupAnswerSignals(openapi::DefaultApi *api, bool benchmark);
    void accept(const openapi::HttpRequestWorker *worker, const openapi::LocalAnswer &answer);
    void reject(const openapi::HttpRequestWorker *worker);
    void complete(const Result &result);
    void finish(const QString &message = {});
    void inspect(const Pending &question, const Result &result);

    BackendSupervisor &backend_;
    BenchmarkSources sources_;
    LocalApiRequest request_{120000};
    LocalApiRequest cancel_;
    QList<BenchmarkSuite> suites_;
    std::optional<BenchmarkSuite> imported_;
    int selectedSuite_ = -1;
    BenchmarkSuite scope_;
    QList<Pending> queue_;
    std::optional<Pending> current_;
    QMap<int, Result> results_;
    AnswerResult result_;
    QString requestId_;
    QString inspectedQuestion_;
    QString inspectedSource_;
    QString traceId_;
    QString feedback_;
    qint64 indexed_ = 0;
    qint64 indexTotal_ = 0;
    int total_ = 0;
    int finished_ = 0;
    int busyRetries_ = 0;
    quint64 runEpoch_ = 0;
    bool hybrid_ = true;
    bool hybridProblem_ = false;
    bool busy_ = false;
    bool stopping_ = false;
};

} // namespace ctxfab
