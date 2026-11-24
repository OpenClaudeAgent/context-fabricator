#include "BackendSupervisor.h"
#include "BenchmarkCatalog.h"
#include "LoopbackApiFixture.h"
#include "PipelineInspector.h"

#include <QFile>
#include <QSettings>
#include <QtTest>

namespace {

using ctxfab::test::Response;
constexpr auto Corpus = "/v1/benchmarks/corpus";
constexpr auto Downloads = "/v1/benchmarks/actions/downloads";
constexpr auto Recorded = "/v1/benchmarks/actions";
constexpr auto Embeddings = "/v1/benchmarks/corpus/embeddings";
constexpr auto BenchmarkAnswers = "/v1/benchmarks/answers";
constexpr auto Answers = "/v1/answers";

QByteArray route(const char *method, const char *path) { return QByteArray(method) + ' ' + path; }

Response list(QJsonArray values = {}) { return {.body = {}, .array = std::move(values)}; }

QJsonObject stored(const ctxfab::openapi::BenchmarkReference &pin, bool active = true) {
    auto result = pin.asJsonObject();
    result.insert("active", active);
    result.insert("files", 42);
    result.insert("bytes", 4096);
    return result;
}

Response progress(const char *state, int indexed) {
    return {{{"state", state}, {"dimensions", 16}, {"indexed", indexed}, {"total", 8}}};
}

Response answer(const char *mode = "bm25", const char *outcome = "answered") {
    return {{{"model", "fixture"},
             {"mode", mode},
             {"outcome", outcome},
             {"text", "Fixture answer"},
             {"citations", QJsonArray{}}}};
}

Response problem(const char *code, const char *message) {
    return {{{"code", code}, {"message", message}, {"status", 503}}, "503 Service Unavailable"};
}

bool writeSuite(QFile &file, const QJsonObject &object) {
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    const auto bytes = QJsonDocument(object).toJson();
    const bool written = file.write(bytes) == bytes.size();
    file.close();
    return written;
}

struct Fixture {
    ctxfab::test::LoopbackApiFixture api;
    QTemporaryDir profile;
    QSettings settings{profile.filePath("preferences.ini"), QSettings::IniFormat};
    ctxfab::BackendSupervisor backend{
        {.executable = {}, .directory = profile.path(), .externalEndpoint = api.endpointPath()}};
    ctxfab::PipelineInspector inspector{backend, settings};
    ctxfab::BenchmarkCatalog catalog = ctxfab::BenchmarkCatalog::bundled();

    void open(QJsonArray corpora = {}, QJsonArray downloads = {}) {
        api.replies[route("GET", Corpus)] = {list(std::move(corpora))};
        api.replies[route("GET", Downloads)] = {list(std::move(downloads))};
        QVERIFY(api.start());
        backend.start();
        QTRY_VERIFY(backend.session());
        inspector.open();
        QTRY_VERIFY(!inspector.sources()->busy());
    }
};

QJsonObject download(const ctxfab::openapi::BenchmarkReference &reference,
                     qint64 runId = 9007199254740993, int attempt = 2) {
    const QJsonObject run{{"id", runId},
                          {"attempt", attempt},
                          {"workflowId", 23},
                          {"event", "pull_request"},
                          {"headSha", reference.getRevision()},
                          {"headBranch", "main"},
                          {"headRepositoryId", 88},
                          {"commitProvenance", "reportedOnly"},
                          {"status", "completed"},
                          {"conclusion", "success"},
                          {"createdAt", "2026-11-24T10:00:00Z"},
                          {"updatedAt", "2026-11-24T10:00:00Z"}};
    const QJsonObject workflow{{"id", 23},
                               {"name", "CI"},
                               {"path", ".github/workflows/ci.yml"},
                               {"state", "active"},
                               {"definitionBlob", QJsonValue::Null},
                               {"definition", QJsonValue::Null}};
    const QJsonObject snapshot{{"repositoryId", 88},
                               {"repository", reference.getRepository()},
                               {"branch", "main"},
                               {"definitionCommit", reference.getRevision()},
                               {"workflowCoverage", "observedRunMetadata"},
                               {"executedDefinition", "unknown"},
                               {"runCoverage", "returnedCommitRuns"},
                               {"attemptCoverage", "observedLatestAttemptsOnly"},
                               {"workflows", QJsonArray{workflow}},
                               {"runs", QJsonArray{run}}};
    return {{"reference", reference.asJsonObject()}, {"snapshots", QJsonArray{snapshot}}};
}

} // namespace

class PipelineTests : public QObject {
    Q_OBJECT

  private slots:
    void loadsSharedCatalogueWithoutDownloading() {
        Fixture fixture;
        fixture.open();
        QCOMPARE(fixture.catalog.corpora.size(), 9);
        int questions = 0;
        for (const auto &suite : fixture.catalog.corpora) {
            QCOMPARE(suite.reference->getRevision().size(), 40);
            questions += suite.questions.size();
        }
        QCOMPARE(questions, 180);
        QCOMPARE(fixture.catalog.captures.size(), 15);
        for (const auto &entry : fixture.catalog.captures) {
            QVERIFY(entry.action);
            QVERIFY(!entry.query.contains(QString::number(entry.action->getRunId())));
        }
        QCOMPARE(fixture.api.requests, (QList<QByteArray>{Corpus, Downloads}));
        fixture.inspector.selectDefaultBenchmark();
        QVERIFY(!fixture.inspector.rows().isEmpty());
        QVERIFY(!fixture.inspector.sourceReady());
        fixture.inspector.runAll();
        QVERIFY(!fixture.inspector.busy());
        QCOMPARE(fixture.api.requests.size(), 2);
    }

    void keepsSeveralCorporaActiveAndRestoresThem() {
        Fixture fixture;
        const auto first = fixture.catalog.corpora.at(0);
        const auto second = fixture.catalog.corpora.at(1);
        fixture.open({stored(*first.reference), stored(*second.reference)});
        QVERIFY(fixture.inspector.sources()->active(*first.reference));
        QVERIFY(fixture.inspector.sources()->active(*second.reference));
        fixture.api.replies[route("DELETE", Corpus)] = {{{}, "204 No Content"}};
        fixture.inspector.sources()->setActive(first.id, false);
        QTRY_VERIFY(!fixture.inspector.sources()->busy());
        QVERIFY(!fixture.inspector.sources()->active(*first.reference));
        QVERIFY(fixture.inspector.sources()->active(*second.reference));
        QCOMPARE(fixture.api.requestBodies.value(route("DELETE", Corpus)),
                 first.reference->asJsonObject());
        fixture.api.replies[route("POST", Corpus)] = {{stored(*first.reference)}};
        fixture.inspector.sources()->setActive(first.id, true);
        QTRY_VERIFY(fixture.inspector.sources()->active(*first.reference));
        QCOMPARE(fixture.api.requestBodies.value(route("POST", Corpus)),
                 first.reference->asJsonObject());
    }

    void runsCoreAnswerInspectionWhenSearchingAllSources() {
        Fixture fixture;
        fixture.open();
        auto inspected = answer();
        inspected.body.insert(
            "evidenceReview",
            QJsonObject{
                {"question", "How is the branch indexed?"},
                {"searches",
                 QJsonArray{QJsonObject{{"source", "localPassages"}, {"candidateCount", 1}}}},
                {"candidates", QJsonArray{QJsonObject{{"source", "localPassages"},
                                                      {"reference", "Policy"},
                                                      {"disposition", "candidateLimit"}}}},
                {"reason", "verified"},
                {"pipeline", QJsonObject{{"fallback", "Embedding model unavailable"},
                                         {"completionInput", "Exact prompt"},
                                         {"draft", QJsonObject{{"answer", "Fixture draft"},
                                                               {"citations", QJsonArray{}}}}}}});
        fixture.api.replies[route("POST", Answers)] = {inspected};
        fixture.inspector.ask("How is the branch indexed?", -1);
        QTRY_VERIFY(fixture.inspector.result().present());
        QCOMPARE(fixture.inspector.inspectedQuestion(),
                 QStringLiteral("How is the branch indexed?"));
        const auto request = fixture.api.requestBodies.value(route("POST", Answers));
        QVERIFY(request.value("inspectPipeline").toBool());
        QVERIFY(!request.value("requestId").toString().isEmpty());
        QCOMPARE(fixture.api.requests.count(BenchmarkAnswers), 0);
        QCOMPARE(fixture.inspector.stageText(0), QStringLiteral("All sources"));
        QVERIFY(fixture.inspector.stageText(1).contains("localPassages"));
        QCOMPARE(fixture.inspector.stageText(3), QStringLiteral("Embedding model unavailable"));
        QVERIFY(fixture.inspector.stageText(4).contains("Policy"));
        QCOMPARE(fixture.inspector.stageText(5), QStringLiteral("Exact prompt"));
        QVERIFY(fixture.inspector.stageText(6).contains("Fixture draft"));
        QVERIFY(fixture.inspector.stageText(7).contains("verified"));
        QVERIFY(fixture.inspector.stageText(-1).isEmpty());
    }

    void indexesHybridBeforeSendingQuestion() {
        Fixture fixture;
        fixture.open({stored(*fixture.catalog.corpora.first().reference)});
        fixture.inspector.selectDefaultBenchmark();
        fixture.api.replies[route("POST", Embeddings)] = {problem("embedding_busy", "Worker busy"),
                                                          progress("incomplete", 4),
                                                          progress("ready", 8)};
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {answer("hybrid")};
        fixture.inspector.runQuestion(0);
        QTRY_COMPARE(fixture.inspector.finishedCount(), 1);
        QTRY_VERIFY(!fixture.inspector.busy());
        QCOMPARE(fixture.api.requests.mid(2),
                 (QList<QByteArray>{Embeddings, Embeddings, Embeddings, BenchmarkAnswers}));
        const auto request = fixture.api.requestBodies.value(route("POST", BenchmarkAnswers));
        QCOMPARE(request.value("retrieval").toString(), QStringLiteral("hybrid"));
        QCOMPARE(request.value("reference").toObject(),
                 fixture.catalog.corpora.first().reference->asJsonObject());
        fixture.inspector.inspectResult(0);
        QVERIFY(fixture.inspector.result().present());
        QCOMPARE(fixture.api.requests.count(BenchmarkAnswers), 1);
    }

    void exposesHybridFailureAndPermitsExplicitBm25Retry_data() {
        QTest::addColumn<bool>("rejectedRequest");
        QTest::newRow("unconfigured-index") << false;
        QTest::newRow("unavailable-runtime") << true;
    }

    void exposesHybridFailureAndPermitsExplicitBm25Retry() {
        QFETCH(bool, rejectedRequest);
        Fixture fixture;
        fixture.open({stored(*fixture.catalog.corpora.first().reference)});
        fixture.inspector.selectDefaultBenchmark();
        const auto unavailable = rejectedRequest ? problem("embedding_runtime_unavailable",
                                                           "Embedding runtime unavailable")
                                                 : progress("notConfigured", 0);
        fixture.api.replies[route("POST", Embeddings)] = {unavailable};
        fixture.inspector.runQuestion(0);
        QTRY_VERIFY(fixture.inspector.hybridProblem());
        QVERIFY(!fixture.inspector.busy());
        QCOMPARE(fixture.api.requests.count(BenchmarkAnswers), 0);
        fixture.inspector.setHybrid(false);
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {answer()};
        fixture.inspector.runQuestion(0);
        QTRY_COMPARE(fixture.inspector.finishedCount(), 1);
        QCOMPARE(
            fixture.api.requestBodies.value(route("POST", BenchmarkAnswers)).value("retrieval"),
            QJsonValue("bm25"));
        QCOMPARE(fixture.api.requests.count(Embeddings), 1);
    }

    void reportsErrorsAndAbstentionsWithoutAbortingSuite() {
        Fixture fixture;
        fixture.open({stored(*fixture.catalog.corpora.first().reference)});
        fixture.inspector.selectDefaultBenchmark();
        fixture.inspector.setHybrid(false);
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {
            problem("unavailable", "Worker unavailable"), answer("bm25", "insufficientEvidence"),
            answer()};
        const auto questionCount = fixture.inspector.rows().size();
        QVERIFY(questionCount > 2);
        fixture.inspector.runAll();
        QTRY_VERIFY(!fixture.inspector.busy());
        QCOMPARE(fixture.inspector.finishedCount(), questionCount);
        QCOMPARE(fixture.api.requests.count(BenchmarkAnswers), questionCount);
        const auto results = fixture.inspector.rows();
        QVERIFY(!results.at(0).error.isEmpty());
        QCOMPARE(results.at(1).outcome, QStringLiteral("insufficientEvidence"));
        QCOMPARE(results.last().outcome, QStringLiteral("answered"));
    }

    void restoresWorkflowDownloadsAndUsesExactAttempt() {
        Fixture fixture;
        const auto pin = *fixture.catalog.corpora.first().reference;
        fixture.open({}, {download(pin)});
        auto *sources = fixture.inspector.sources();
        const auto workflow = sources->workflows().constFirst();
        QVERIFY(workflow.active);
        QVERIFY(workflow.downloaded);
        QVERIFY(!sources->loadedCaptures());
        QCOMPARE(fixture.api.requests, (QList<QByteArray>{Corpus, Downloads}));
        const auto workflowId = workflow.key;
        sources->setWorkflowActive(workflowId, false);
        QVERIFY(!sources->workflows().constFirst().active);
        ctxfab::PipelineInspector reopened(fixture.backend, fixture.settings);
        reopened.open();
        QTRY_VERIFY(!reopened.sources()->workflows().isEmpty());
        QVERIFY(!reopened.sources()->workflows().constFirst().active);
        sources->setWorkflowActive(workflowId, true);
        fixture.inspector.selectSuite(static_cast<int>(fixture.inspector.suites().size() - 1));
        QVERIFY(fixture.inspector.sourceReady());
        QVERIFY(fixture.inspector.rows().size() >= 3);
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {answer("selected")};
        fixture.inspector.runQuestion(0);
        QTRY_COMPARE(fixture.inspector.finishedCount(), 1);
        const auto action = fixture.api.requestBodies.value(route("POST", BenchmarkAnswers))
                                .value("action")
                                .toObject();
        QCOMPARE(action.value("runId").toInteger(), 9007199254740993LL);
        QCOMPARE(action.value("attempt").toInt(), 2);
        QVERIFY(action.value("downloaded").toBool());
    }

    void optsInToRecordedCapturesAndRestoresChoice() {
        Fixture fixture;
        fixture.open();
        const auto recorded = *fixture.catalog.captures.first().action;
        ctxfab::openapi::BenchmarkReference pin;
        pin.setRepository(recorded.getRepository());
        pin.setRevision(QString(40, 'a'));
        const auto captures =
            download(pin, recorded.getRunId(), recorded.getAttempt()).value("snapshots").toArray();
        fixture.api.replies[route("GET", Recorded)] = {list(captures)};
        QVERIFY(!fixture.inspector.sources()->loadedCaptures());
        fixture.inspector.sources()->loadRecordedCaptures();
        QTRY_VERIFY(fixture.inspector.sources()->loadedCaptures());
        fixture.inspector.selectDefaultBenchmark();
        QVERIFY(fixture.inspector.sourceReady());
        QTRY_VERIFY(!fixture.inspector.rows().isEmpty());
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {answer("selected")};
        fixture.inspector.runQuestion(0);
        QTRY_COMPARE(fixture.inspector.finishedCount(), 1);
        const auto action = fixture.api.requestBodies.value(route("POST", BenchmarkAnswers))
                                .value("action")
                                .toObject();
        QCOMPARE(action.value("runId").toInteger(), recorded.getRunId());
        QVERIFY(!action.value("downloaded").toBool());
        ctxfab::PipelineInspector reopened(fixture.backend, fixture.settings);
        reopened.open();
        QTRY_VERIFY(reopened.sources()->loadedCaptures());
        QCOMPARE(fixture.api.requests.count(Recorded), 2);
    }

    void inspectsConnectedWorkflowWithCoreAnswerRoute() {
        Fixture fixture;
        fixture.open();
        const auto reference = *fixture.catalog.corpora.first().reference;
        const auto snapshot = download(reference).value("snapshots").toArray().first();
        const QJsonObject features{
            {"currentCode", true}, {"history", false}, {"actions", true}, {"codeGraph", false}};
        const QJsonObject repo{{"id", 88},
                               {"fullName", reference.getRepository()},
                               {"defaultBranch", "main"},
                               {"private", false}};
        const QJsonObject config{{"repository", repo},      {"branch", "main"},
                                 {"intervalMinutes", 60},   {"features", features},
                                 {"contents", "available"}, {"actions", "available"},
                                 {"verified", true}};
        const QJsonObject defaults{{"intervalMinutes", 60}, {"features", features}};
        fixture.api.replies["GET /v1/github"] = {
            {{{"revision", 1},
              {"credential", "connected"},
              {"authorizationUrl", "https://github.com"},
              {"actionsAuthorizationUrl", "https://github.com"},
              {"defaults", defaults},
              {"repositories", QJsonArray{config}}}}};
        fixture.api.replies["GET /v1/github/repositories/88/actions"] = {
            {{{"availability", "ready"}, {"snapshot", snapshot}}}};
        fixture.inspector.sources()->loadConnectedSources();
        QTRY_COMPARE(fixture.inspector.sources()->connectedWorkflows().size(), 1);
        fixture.inspector.selectSuite(static_cast<int>(fixture.inspector.suites().size() - 1));
        fixture.api.replies[route("POST", Answers)] = {answer("selected")};
        fixture.inspector.ask("Why did this workflow run?", -1);
        QTRY_VERIFY(fixture.inspector.result().present());
        const auto request = fixture.api.requestBodies.value(route("POST", Answers));
        QCOMPARE(request.value("repositoryId").toInteger(), 88);
        QCOMPARE(request.value("actionsRunId").toInteger(), 9007199254740993LL);
        QCOMPARE(request.value("actionsRunAttempt").toInt(), 2);
        QCOMPARE(request.value("branch"), QJsonValue("main"));
        QVERIFY(request.value("inspectPipeline").toBool());
        QCOMPARE(fixture.api.requests.count(BenchmarkAnswers), 0);
    }

    void downloadsAllActionRefsAfterOneFails() {
        Fixture fixture;
        fixture.open();
        QList<Response> replies{problem("unavailable", "Download failed")};
        for (const auto &suite : fixture.catalog.corpora) {
            replies.append(
                {{{"reference", suite.reference->asJsonObject()}, {"snapshots", QJsonArray{}}}});
        }
        fixture.api.replies[route("POST", Downloads)] = replies;
        fixture.inspector.sources()->downloadAllActions();
        QTRY_VERIFY(!fixture.inspector.sources()->busy());
        QCOMPARE(fixture.api.requests.count(Downloads), 1 + fixture.catalog.corpora.size());
        QVERIFY(!fixture.inspector.sources()->feedback().isEmpty());
        QVERIFY(fixture.inspector.sources()->workflows().isEmpty());
    }

    void stopsInFlightAnswerAndDoesNotSubmitRemainder() {
        Fixture fixture;
        fixture.open({stored(*fixture.catalog.corpora.first().reference)});
        fixture.inspector.selectDefaultBenchmark();
        fixture.inspector.setHybrid(false);
        auto pending = answer();
        pending.delayMs = 3000;
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {pending};
        fixture.api.replies[route("DELETE", "/v1/answers/*")] = {
            {{{"requestId", "00000000-0000-0000-0000-000000000001"}, {"status", "cancelled"}}}};
        fixture.inspector.runAll();
        QTRY_COMPARE(fixture.api.requests.count(BenchmarkAnswers), 1);
        fixture.inspector.stop();
        QTRY_VERIFY(!fixture.inspector.busy());
        const auto requestId = fixture.api.requestBodies.value(route("POST", BenchmarkAnswers))
                                   .value("requestId")
                                   .toString()
                                   .toLatin1();
        QVERIFY(!requestId.isEmpty());
        QCOMPARE(fixture.api.requests.last(), QByteArray("/v1/answers/") + requestId);
        QCOMPARE(fixture.inspector.finishedCount(), 0);
        QCOMPARE(fixture.api.requests.count(BenchmarkAnswers), 1);
    }

    void activatesAllCorporaWithoutDownloadingAlreadyActiveRefs() {
        Fixture fixture;
        const auto first = fixture.catalog.corpora.first();
        fixture.open({stored(*first.reference)});
        QList<Response> replies;
        for (const auto &suite : fixture.catalog.corpora.sliced(1)) {
            replies.append({stored(*suite.reference)});
        }
        fixture.api.replies[route("POST", Corpus)] = replies;
        fixture.inspector.sources()->activateAllCorpora();
        QTRY_VERIFY(!fixture.inspector.sources()->busy());
        QCOMPARE(fixture.api.requests.count(Corpus), fixture.catalog.corpora.size());
        QVERIFY(!fixture.inspector.sources()->canActivateCorpora());
        for (const auto &suite : fixture.catalog.corpora) {
            QVERIFY(fixture.inspector.sources()->active(*suite.reference));
        }
    }

    void restoresCustomCorporaAndRetriesFailedActivations() {
        Fixture fixture;
        auto saved = *fixture.catalog.corpora.first().reference;
        saved.setRepository("example/java-fixture");
        fixture.open({stored(saved)});
        auto *sources = fixture.inspector.sources();
        const auto savedKey = ctxfab::BenchmarkCatalog::key(saved);
        QVERIFY(sources->saved(savedKey));
        QVERIFY(sources->active(saved));

        const auto first = fixture.catalog.corpora.first();
        QList<Response> replies{problem("unavailable", "Corpus unavailable")};
        for (const auto &suite : fixture.catalog.corpora.sliced(1)) {
            replies.append({stored(*suite.reference)});
        }
        fixture.api.replies[route("POST", Corpus)] = replies;
        sources->activateAllCorpora();
        QTRY_VERIFY(!sources->busy());
        QVERIFY(!sources->active(*first.reference));
        QVERIFY(sources->active(*fixture.catalog.corpora.last().reference));
        QVERIFY(sources->feedback().contains(first.reference->getRepository()));
        QVERIFY(sources->feedback().contains("Corpus unavailable"));

        fixture.api.replies[route("POST", Corpus)] = {{stored(*first.reference)}};
        sources->setActive(first.id, true);
        QTRY_VERIFY(sources->active(*first.reference));
        QVERIFY(sources->feedback().isEmpty());

        fixture.api.replies[route("POST", Downloads)] = {{download(saved)}};
        sources->downloadActions(savedKey);
        QTRY_VERIFY(!sources->busy());
        QCOMPARE(fixture.api.requestBodies.value(route("POST", Downloads)), saved.asJsonObject());
        QCOMPARE(sources->workflows().size(), 1);
        QVERIFY(sources->corpora().last().hasActions);
        QCOMPARE(sources->corpora().last().runCount, 1);
    }

    void keepsOfflineSourceActionsIdle() {
        Fixture fixture;
        auto *sources = fixture.inspector.sources();
        const auto first = fixture.catalog.corpora.first();
        fixture.inspector.open();
        sources->setActive(first.id, true);
        sources->activateAllCorpora();
        sources->downloadActions(first.id);
        sources->loadRecordedCaptures();
        sources->loadConnectedSources();
        QVERIFY(!sources->busy());
        QVERIFY(!sources->active(*first.reference));
        QVERIFY(!sources->feedback().isEmpty());
        QVERIFY(fixture.api.requests.isEmpty());
    }

    void stopsPollingWhenIndexMakesNoProgress() {
        Fixture fixture;
        fixture.open({stored(*fixture.catalog.corpora.first().reference)});
        fixture.inspector.selectDefaultBenchmark();
        fixture.api.replies[route("POST", Embeddings)] = {progress("incomplete", 4)};
        fixture.inspector.runQuestion(0);
        QTRY_VERIFY(!fixture.inspector.busy());
        QCOMPARE(fixture.api.requests.count(Embeddings), 2);
        QCOMPARE(fixture.api.requests.count(BenchmarkAnswers), 0);
        QVERIFY(!fixture.inspector.feedback().isEmpty());
    }

    void acceptsJsonSuiteAndRejectsInvalidReference() {
        Fixture fixture;
        fixture.open();
        QFile file(fixture.profile.filePath("suite.json"));
        const QJsonArray questions{QJsonObject{
            {"title", "Retry delay"}, {"query", "When do we retry?"}, {"expected", "47 seconds."}}};
        const QJsonArray documents{
            QJsonObject{{"path", "docs/policy.txt"}, {"text", "Retry after 47 seconds."}}};
        QVERIFY(writeSuite(file, {{"questions", questions}, {"documents", documents}}));
        fixture.inspector.importSuite(QUrl::fromLocalFile(file.fileName()));
        QVERIFY(fixture.inspector.sourceReady());
        QCOMPARE(fixture.inspector.rows().constFirst().expected, QStringLiteral("47 seconds."));
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {answer()};
        fixture.inspector.runAll();
        QTRY_COMPARE(fixture.inspector.finishedCount(), 1);
        QCOMPARE(
            fixture.api.requestBodies.value(route("POST", BenchmarkAnswers)).value("documents"),
            QJsonValue(documents));
        const QJsonObject invalid{{"repository", "owner/repo"}, {"revision", "main"}};
        QVERIFY(writeSuite(file, {{"questions", questions}, {"reference", invalid}}));
        fixture.inspector.importSuite(QUrl::fromLocalFile(file.fileName()));
        QVERIFY(!fixture.inspector.feedback().isEmpty());
        QCOMPARE(fixture.inspector.rows().size(), 1);
        QCOMPARE(fixture.api.requests.count(Corpus), 1);
    }

    void mergesImportedQuestionsIntoTheSamePinnedCorpus() {
        Fixture fixture;
        fixture.open();
        auto reference = *fixture.catalog.corpora.first().reference;
        reference.setRepository("example/fixture");
        const QJsonObject first{{"title", "Branch"}, {"query", "Which branch is indexed?"}};
        const QJsonObject second{{"title", "Index"}, {"query", "Where is the index stored?"}};
        QFile file(fixture.profile.filePath("suite.json"));
        const auto url = QUrl::fromLocalFile(file.fileName());
        QVERIFY(writeSuite(file,
                           {{"questions", QJsonArray{}}, {"reference", reference.asJsonObject()}}));
        fixture.inspector.importSuite(url);
        QVERIFY(!fixture.inspector.feedback().isEmpty());

        QVERIFY(writeSuite(
            file, {{"questions", QJsonArray{first}}, {"reference", reference.asJsonObject()}}));
        fixture.inspector.importSuite(url);
        QCOMPARE(fixture.inspector.selectedCorpus(), ctxfab::BenchmarkCatalog::key(reference));
        QCOMPARE(fixture.inspector.rows().size(), 1);
        QVERIFY(!fixture.inspector.sourceReady());

        QVERIFY(writeSuite(file, {{"questions", QJsonArray{first, second}},
                                  {"reference", reference.asJsonObject()}}));
        fixture.inspector.importSuite(url);
        QCOMPARE(fixture.inspector.rows().size(), 2);
        QCOMPARE(fixture.inspector.rows().last().query, second.value("query").toString());
        QCOMPARE(fixture.api.requests.count(Corpus), 1);
    }

    void acceptsTextImportAndRejectsBinary() {
        Fixture fixture;
        fixture.open();
        QFile file(fixture.profile.filePath("policy.txt"));
        QVERIFY(file.open(QIODevice::WriteOnly));
        QVERIFY(file.write("Retry after 47 seconds.\n") > 0);
        file.close();
        fixture.inspector.importFiles({QUrl::fromLocalFile(file.fileName())});
        QVERIFY(fixture.inspector.sourceReady());
        fixture.api.replies[route("POST", BenchmarkAnswers)] = {answer()};
        fixture.inspector.ask("When do we retry?", -1);
        QTRY_VERIFY(fixture.inspector.result().present());
        const auto request = fixture.api.requestBodies.value(route("POST", BenchmarkAnswers));
        QCOMPARE(request.value("documents").toArray().first().toObject().value("path"),
                 QJsonValue("policy.txt"));
        fixture.inspector.importFiles({QUrl::fromLocalFile(file.fileName())});
        QVERIFY(!fixture.inspector.feedback().isEmpty());

        QFile binary(fixture.profile.filePath("data.bin"));
        QVERIFY(binary.open(QIODevice::WriteOnly));
        QCOMPARE(binary.write(QByteArray::fromHex("fffe00")), 3);
        binary.close();
        const auto suite = fixture.inspector.selectedSuite();
        fixture.inspector.importFiles({QUrl::fromLocalFile(binary.fileName())});
        QVERIFY(!fixture.inspector.feedback().isEmpty());
        QCOMPARE(fixture.inspector.selectedSuite(), suite);
    }
};

QTEST_GUILESS_MAIN(PipelineTests)
#include "PipelineTests.moc"
