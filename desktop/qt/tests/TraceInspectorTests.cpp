#include "AnswerUsageModel.h"
#include "BackendSupervisor.h"
#include "DiagnosticLogs.h"
#include "Diagnostics.h"
#include "LoopbackApiFixture.h"
#include "TraceInspector.h"

#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QTemporaryDir>
#include <QUrl>
#include <QtTest>

namespace {

const QString TraceA(32, 'a');
const QString TraceB(32, 'b');
const QString Parent(16, 'c');
const QString Child(16, 'd');
constexpr qint64 Cursor = 9'007'199'254'740'995LL;

QJsonObject sample(qint64 id, const QString &trace, const QString &name, const QString &span) {
    return {{"id", id},
            {"timestamp", id > Cursor ? 21 : 20},
            {"name", name},
            {"traceId", trace},
            {"spanId", span}};
}

QJsonObject page(const QJsonArray &spans, const QJsonArray &logs) {
    return {{"spans", spans}, {"logs", logs}};
}

} // namespace

class TraceInspectorTests : public QObject {
    Q_OBJECT

  private slots:
    void readsRecordedAnswerDetailsFromTheSelectedSpan() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        auto span = sample(2, TraceA, "answer_retrieval", Child);
        span.insert("spanOutcome", "insufficientEvidence");
        span.insert("events",
                    QJsonArray{QJsonObject{{"name", "No evidence found"}, {"offsetMicros", 0}}});
        span.insert("clientKind", "desktop");
        span.insert("modelProfile", "qwen-test");
        span.insert("contextBudget", QJsonObject{{"kind", "passages"},
                                                 {"proposed", 3},
                                                 {"admitted", 2},
                                                 {"inputTokens", 1024},
                                                 {"inputTokenLimit", 3455},
                                                 {"measurements", 2}});
        span.insert("inferenceTiming", QJsonObject{{"cachedPromptTokens", 0},
                                                   {"processedPromptTokens", 1024},
                                                   {"generatedTokens", 17},
                                                   {"promptMicros", 243136},
                                                   {"generationMicros", Cursor}});
        span.insert("retrievalCounts", QJsonObject{{"passageCandidates", 3},
                                                   {"historyPairs", 0},
                                                   {"workflowAttempts", 0},
                                                   {"proposedSources", 3}});
        span.insert("fusionRanking",
                    QJsonObject{{"rankOffset", 60},
                                {"denseWeight", 1.0},
                                {"lexicalCandidates", 1},
                                {"denseCandidates", 2},
                                {"candidates", QJsonArray{
                                                   QJsonObject{{"scoreRank", 1},
                                                               {"lexicalRank", 1},
                                                               {"denseRank", 2},
                                                               {"score", 0.03},
                                                               {"finalRank", 1},
                                                               {"lexicalPromoted", true}},
                                                   QJsonObject{{"scoreRank", 2},
                                                               {"score", 0.01},
                                                               {"lexicalPromoted", false}},
                                               }}});
        auto other = sample(1, TraceA, "api_answer", Parent);
        other.insert("contextBudget", QJsonObject{{"kind", "historyPairs"},
                                                  {"proposed", 1},
                                                  {"admitted", 0},
                                                  {"inputTokenLimit", 3455},
                                                  {"measurements", 1}});
        fixture.replies.insert("/v1/telemetry/traces/" + TraceA.toLatin1(),
                               {{.body = page({span, other}, {})}});
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::TraceInspector inspector(backend);
        const ctxfab::TraceRecord empty;
        QVERIFY(empty.answerDetails().budgetSummary().isEmpty());
        QVERIFY(empty.answerDetails().tokenBudget().isEmpty());
        QVERIFY(empty.answerDetails().tokenChecks().isEmpty());
        QVERIFY(empty.answerDetails().inference().isEmpty());
        QVERIFY(empty.answerDetails().retrieval().isEmpty());
        QVERIFY(empty.answerDetails().fusion().isEmpty());
        QVERIFY(empty.answerDetails().fusionCandidates().isEmpty());
        QVERIFY(empty.client().isEmpty());
        inspector.open(TraceA, Child);
        QTRY_COMPARE(inspector.selected().spanId(), Child);
        QVERIFY(inspector.isAnswerTrace());
        QVERIFY(!inspector.opensRoot());
        QVERIFY(inspector.recordedAbstention());
        QCOMPARE(inspector.recordedRetrieval().size(), 4);
        QCOMPARE(inspector.abstentionDetail(),
                 QStringLiteral("The recorded retrieval step found no usable evidence."));
        QCOMPARE(inspector.selected().client(), QStringLiteral("Desktop"));
        QCOMPARE(inspector.selected().modelProfile(), QStringLiteral("qwen-test"));
        const auto detail = inspector.selected().answerDetails();
        QCOMPARE(detail.budgetSummary(), QStringLiteral("2 of 3 passages selected"));
        QVERIFY(detail.tokenBudget().endsWith("input tokens"));
        QCOMPARE(detail.tokenChecks(), QStringLiteral("2"));
        QCOMPARE(detail.inference().size(), 5);
        QCOMPARE(detail.inference().constFirst().value, QStringLiteral("243.136 ms"));
        QCOMPARE(detail.retrieval().size(), 4);
        QCOMPARE(detail.retrieval().at(1).value, QStringLiteral("0"));
        QCOMPARE(detail.fusion().size(), 4);
        QCOMPARE(detail.fusionCandidates().size(), 2);
        QVERIFY(detail.fusionCandidates().constFirst().value.contains("Lexical #1"));
        QVERIFY(detail.fusionCandidates().constFirst().title.contains("Lexical promotion"));
        QVERIFY(detail.fusionCandidates().constLast().title.contains("Dropped at top-20 cutoff"));
        inspector.selectSpan(inspector.selectedIndex() == 0 ? 1 : 0);
        QCOMPARE(inspector.selected().spanId(), Parent);
        const auto unavailable = inspector.selected().answerDetails();
        QCOMPARE(unavailable.budgetSummary(), QStringLiteral("0 of 1 history pairs selected"));
        QVERIFY(unavailable.tokenBudget().startsWith("No evidence fit within"));
    }

    void pagesAnswerRequestsWithoutMixingInDebugLogs() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        auto recent = sample(Cursor + 5, TraceA, "api_answer", Child);
        recent.insert("level", "error");
        auto debug = sample(Cursor + 4, TraceA, "api_answer", Child);
        debug.insert("level", "debug");
        auto noTrace = sample(Cursor + 3, TraceA, "api_answer", Child);
        noTrace.remove("traceId");
        const auto unrelated = sample(Cursor, TraceA, "search", Child);
        auto older = sample(Cursor - 1, TraceB, "api_answer", Parent);
        older.insert("level", "info");
        const QByteArray route = "/v1/telemetry/logs?query=api_answer";
        fixture.replies.insert(route,
                               {{.body = {
                                     {"records", QJsonArray{recent, debug, noTrace, unrelated}},
                                     {"nextBeforeId", Cursor},
                                 }}});
        fixture.replies.insert(route + "&beforeId=" + QByteArray::number(Cursor),
                               {{.body = {{"records", QJsonArray{older}}}}});
        fixture.replies.insert("/v1/telemetry/traces/" + TraceB.toLatin1(),
                               {{.body = page({older}, {})}});
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::DiagnosticLogs requests(backend, ctxfab::DiagnosticLogs::Feed::AnswerRequests);
        requests.setActive(true);
        QTRY_COMPARE(requests.rowCount(), 1);
        QVERIFY(requests.hasMore());
        QCOMPARE(fixture.requests, (QList<QByteArray>{route}));
        const int role = requests.roleNames().key("sample");
        QVERIFY(
            qvariant_cast<ctxfab::TraceRecord>(requests.data(requests.index(0), role)).isError());
        requests.setLevel(ctxfab::DiagnosticLogs::Debug);
        requests.setQuery("search");
        QCOMPARE(requests.query(), QStringLiteral("api_answer"));
        QVERIFY(requests.allLevels());
        requests.loadMore();
        QTRY_COMPARE(requests.rowCount(), 2);
        QVERIFY(!requests.hasMore());
        QCOMPARE(fixture.requests.last(), route + "&beforeId=" + QByteArray::number(Cursor));
        const auto record =
            qvariant_cast<ctxfab::TraceRecord>(requests.data(requests.index(1), role));
        QVERIFY(!record.isError());
        ctxfab::TraceInspector inspector(backend);
        inspector.open(record.traceId(), {});
        QTRY_COMPARE(inspector.selected().spanId(), Parent);
        QCOMPARE(inspector.traceId(), TraceB);

        QList<int> refreshedCounts;
        const auto resetObserver = connect(&requests, &QAbstractItemModel::modelReset, &requests,
                                           [&] { refreshedCounts.append(requests.rowCount()); });
        requests.setActive(false);
        requests.setActive(true);
        QTRY_COMPARE(refreshedCounts, QList<int>{1});
        QVERIFY(requests.hasMore());
        disconnect(resetObserver);
    }

    void refreshesUsageAndPreservesTheLastSuccessfulReading() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        const QJsonObject desktop{
            {"client", "desktop"},  {"modelProfile", "qwen-test"},
            {"published", 3},       {"abstained", 2},
            {"cancelled", 1},       {"failed", 2},
            {"unknown", 0},         {"p50Micros", 243136},
            {"p95Micros", 1012000}, {"latestTraceId", TraceA},
        };
        const QJsonObject cli{{"client", "cli"}, {"published", 0}, {"abstained", 0},
                              {"cancelled", 0},  {"failed", 1},    {"unknown", 1}};
        const QJsonObject other{{"client", "other"}, {"published", 0}, {"abstained", 1},
                                {"cancelled", 0},    {"failed", 0},    {"unknown", 0}};
        fixture.replies.insert(
            "/v1/telemetry/answer-usage",
            {{.body = {{"groups", QJsonArray{}}}},
             {.body = {{"firstAt", 100},
                       {"lastAt", 400},
                       {"groups", QJsonArray{desktop, cli, other}}}},
             {.body = {}},
             {.body = {{"code", "busy"}, {"message", "Try again."}}, .status = "503 Unavailable"}});
        fixture.replies.insert("/v1/telemetry/traces/" + TraceA.toLatin1(),
                               {{.body = page({sample(5, TraceA, "api_answer", Parent)}, {})}});
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::AnswerUsageModel usage(backend);
        usage.refresh();
        QVERIFY(!usage.busy());
        usage.setActive(true);
        QTRY_VERIFY(usage.hasSnapshot());
        QVERIFY(usage.groups().isEmpty());
        QVERIFY(usage.period().isEmpty());
        usage.refresh();
        QTRY_COMPARE(usage.groups().size(), 3);
        const auto groups = usage.groups();
        QCOMPARE(groups.at(0).client(), QStringLiteral("Desktop"));
        QCOMPARE(groups.at(0).profile(), QStringLiteral("qwen-test"));
        QCOMPARE(groups.at(0).published(), 3);
        QCOMPARE(groups.at(0).abstained(), 2);
        QCOMPARE(groups.at(0).cancelled(), 1);
        QCOMPARE(groups.at(0).failed(), 2);
        QCOMPARE(groups.at(0).unknown(), 0);
        QCOMPARE(groups.at(0).median(), QStringLiteral("243.136 ms"));
        QCOMPARE(groups.at(0).p95(), QStringLiteral("1.012 s"));
        QCOMPARE(groups.at(1).client(), QStringLiteral("CLI"));
        QCOMPARE(groups.at(1).profile(), QStringLiteral("Model unspecified"));
        QVERIFY(groups.at(1).median().isEmpty() && groups.at(1).p95().isEmpty());
        QVERIFY(groups.at(1).traceId().isEmpty());
        QCOMPARE(groups.at(2).client(), QStringLiteral("Other / untagged"));
        QVERIFY(usage.period().contains(" – "));
        ctxfab::TraceInspector inspector(backend);
        inspector.open(groups.at(0).traceId(), {});
        QTRY_COMPARE(inspector.selected().spanId(), Parent);
        usage.refresh();
        QTRY_COMPARE(usage.feedback(), QStringLiteral("Invalid response from the backend"));
        QCOMPARE(usage.groups().size(), 3);
        usage.refresh();
        QTRY_COMPARE(usage.feedback(), QStringLiteral("Try again."));
        QCOMPARE(usage.groups().size(), 3);
        usage.setActive(false);
        QVERIFY(!usage.hasSnapshot() && !usage.busy() && usage.feedback().isEmpty());
    }

    void exportsOnlyTheTypedTelemetrySnapshot() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        const QTemporaryDir output;
        QVERIFY(output.isValid());
        const auto destination = QUrl::fromLocalFile(output.filePath("diagnostics.json"));
        const QJsonObject snapshot{
            {"logs", QJsonArray{}},
            {"metrics", QJsonArray{}},
            {"traces", QJsonArray{sample(Cursor, TraceA, "search", Parent)}},
            {"health", QJsonObject{{"state", "ready"},
                                   {"dropped", 0},
                                   {"pruned", 0},
                                   {"queued", 0},
                                   {"fileBytes", 10},
                                   {"budgetBytes", 40},
                                   {"retentionHours", 72},
                                   {"debugUntil", 0},
                                   {"oldestAt", 20}}},
        };
        auto incoming = snapshot;
        incoming.insert("unexpected", "backend-only");
        fixture.replies.insert("/v1/telemetry", {{.body = incoming}});
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::Diagnostics diagnostics(backend);
        diagnostics.saveSnapshot(destination);
        QVERIFY(!QFile::exists(destination.toLocalFile()));
        diagnostics.setPageVisible(true);
        QTRY_VERIFY(diagnostics.ready());
        diagnostics.saveSnapshot(QUrl("https://invalid.example/data.json"));
        QCOMPARE(diagnostics.feedback(), QStringLiteral("Couldn’t export diagnostics."));
        diagnostics.saveSnapshot(QUrl::fromLocalFile(output.filePath("missing/diagnostics.json")));
        QCOMPARE(diagnostics.feedback(), QStringLiteral("Couldn’t export diagnostics."));
        diagnostics.saveSnapshot(destination);
        QCOMPARE(diagnostics.feedback(), QStringLiteral("Diagnostics exported."));
        QFile file(destination.toLocalFile());
        QVERIFY(file.open(QIODevice::ReadOnly));
        const auto exported = QJsonDocument::fromJson(file.readAll()).object();
        QCOMPARE(exported, snapshot);
        QCOMPARE(exported["traces"].toArray().at(0).toObject().value("id").toInteger(), Cursor);
    }

    void searchesEventsAndPagesLogsWithExactCursors() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        fixture.replies.insert(
            "/v1/diagnostics",
            {{.body = {},
              .array = QJsonArray{QJsonObject{{"id", 1},
                                              {"timestamp", 20},
                                              {"code", "ready"},
                                              {"message", "Backend is ready."}}}}});
        auto recent = sample(Cursor, TraceA, "recent_log", Child);
        recent.insert("level", "debug");
        auto older = sample(Cursor - 1, TraceA, "older_log", Parent);
        older.insert("level", "debug");
        const auto route = QByteArray("/v1/telemetry/logs");
        fixture.replies.insert(route, {{.body = {{"records", QJsonArray{recent}}}}});
        fixture.replies.insert(route + "?query=ready", {{.body = {{"records", QJsonArray{}}}}});
        fixture.replies.insert(route + "?query=ready&level=debug",
                               {{.body = {{"records", QJsonArray{}}}}});
        const auto filtered = route + "?query=a%20%26%20b&level=debug";
        fixture.replies.insert(
            filtered, {{.body = {{"records", QJsonArray{recent}}, {"nextBeforeId", Cursor}}}});
        fixture.replies.insert(filtered + "&beforeId=" + QByteArray::number(Cursor),
                               {{.body = {{"records", QJsonArray{recent}}}},
                                {.body = {{"records", QJsonArray{older}}}}});
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::DiagnosticLogs logs(backend);
        logs.setActive(true);
        QTRY_COMPARE(logs.rowCount(), 1);
        QTRY_COMPARE(logs.events().size(), 1);
        QCOMPARE(logs.events().constFirst().title(), QStringLiteral("Backend is ready."));
        QVERIFY(!logs.events().constFirst().observedAt().isEmpty());
        QVERIFY(logs.ready());
        logs.setQuery("discard me");
        logs.setQuery("  ready  ");
        QTRY_VERIFY(!logs.busy());
        QCOMPARE(logs.rowCount(), 0);
        QCOMPARE(logs.events().size(), 1);
        QVERIFY(!fixture.requests.contains(route + "?query=discard%20me"));
        logs.setLevel(ctxfab::DiagnosticLogs::Debug);
        QTRY_VERIFY(!logs.busy());
        QVERIFY(logs.events().isEmpty());
        logs.setQuery("a & b");
        QTRY_COMPARE(logs.rowCount(), 1);
        QVERIFY(logs.hasMore());
        const auto sampleRole = logs.roleNames().key("sample");
        const auto record =
            qvariant_cast<ctxfab::TraceRecord>(logs.data(logs.index(0), sampleRole));
        QCOMPARE(record.traceId(), TraceA);
        QCOMPARE(record.spanId(), Child);
        QVERIFY(record.isDebug());
        logs.loadMore();
        QTRY_COMPARE(logs.feedback(), QStringLiteral("Invalid response from the backend"));
        QVERIFY(logs.hasMore());
        QCOMPARE(logs.rowCount(), 1);
        logs.loadMore();
        QTRY_COMPARE(logs.rowCount(), 2);
        QVERIFY(!logs.hasMore());
        QSignalSpy rowsChanged(&logs, &ctxfab::DiagnosticLogs::dataChanged);
        logs.retranslate();
        QCOMPARE(rowsChanged.count(), 1);
        logs.setQuery(QString(129, 'a'));
        QTRY_COMPARE(logs.feedback(), QStringLiteral("Enter a log query of up to 128 characters."));
        fixture.replies.insert(route + "?level=error", {{.body = {{"records", QJsonArray{}}}}});
        logs.setQuery({});
        logs.setLevel(ctxfab::DiagnosticLogs::Error);
        QTRY_VERIFY(fixture.requests.contains(route + "?level=error") && !logs.busy());
        logs.setActive(false);
        logs.setQuery("inactive");
        QVERIFY(!logs.busy());
        QVERIFY(!fixture.requests.contains(route + "?query=inactive&level=error"));
    }

    void reportsAnUnavailableLogSession() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = directory.filePath("missing")});
        ctxfab::DiagnosticLogs logs(backend);
        logs.setActive(true);
        QCOMPARE(logs.feedback(), QStringLiteral("Backend session unavailable"));
        QVERIFY(!logs.ready() && !logs.busy() && logs.events().isEmpty());
    }

    void filtersRecordedTracesAndMetrics() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        auto foreground = sample(23, TraceA, "search", Parent);
        foreground.insert("displayName", "Search request");
        foreground.insert("operation", "GET /v1/search");
        foreground.insert("value", 234);
        auto background = sample(22, TraceB, "api_request", Child);
        background.insert("traceRequestKind", "backgroundCheck");
        auto metric = sample(21, TraceA, "search_requests", Parent);
        metric.insert("value", 42);
        const QJsonObject snapshot{
            {"logs", QJsonArray{}},
            {"metrics", QJsonArray{metric}},
            {"traces", QJsonArray{foreground, background}},
            {"health", QJsonObject{{"state", "gaps"},
                                   {"dropped", 2},
                                   {"pruned", 1},
                                   {"queued", 0},
                                   {"fileBytes", 2000},
                                   {"budgetBytes", 4000},
                                   {"retentionHours", 72},
                                   {"debugUntil", 0},
                                   {"oldestAt", 20}}},
        };
        fixture.replies.insert("/v1/telemetry",
                               {
                                   {.body = {}, .status = "200 OK"},
                                   {.body = snapshot},
                                   {.body = {{"code", "busy"}, {"message", "Try again."}},
                                    .status = "503 Unavailable"},
                               });
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::Diagnostics diagnostics(backend);
        diagnostics.setPageVisible(true);
        QTRY_COMPARE(diagnostics.feedback(), QStringLiteral("Invalid response from the backend"));
        QVERIFY(!diagnostics.ready());
        diagnostics.refresh();
        QTRY_VERIFY(diagnostics.ready());
        QCOMPARE(diagnostics.healthStatus(), QStringLiteral("Data gaps"));
        QCOMPARE(diagnostics.storageFraction(), 0.5);
        QCOMPARE(diagnostics.traces().size(), 1);
        QCOMPARE(diagnostics.traces().constFirst().traceId(), TraceA);
        QCOMPARE(diagnostics.traces().constFirst().spanId(), Parent);
        QCOMPARE(diagnostics.traces().constFirst().duration(), QStringLiteral("234 µs"));
        QVERIFY(diagnostics.traces().constFirst().observedAt().contains(':'));
        QCOMPARE(diagnostics.metrics().constFirst().count(), QStringLiteral("42"));

        diagnostics.setFilter("  gEt /V1/sEaRcH  ");
        QCOMPARE(diagnostics.traces().size(), 1);
        QVERIFY(diagnostics.metrics().isEmpty());
        diagnostics.setFilter(TraceB);
        QVERIFY(diagnostics.traces().isEmpty());
        diagnostics.setIncludeBackgroundChecks(true);
        QCOMPARE(diagnostics.traces().constFirst().traceId(), TraceB);
        diagnostics.setFilter("search_requests");
        QCOMPARE(diagnostics.metrics().size(), 1);
        diagnostics.refresh();
        QTRY_COMPARE(diagnostics.feedback(), QStringLiteral("Try again."));
        QVERIFY(diagnostics.ready());
        QCOMPARE(diagnostics.metrics().size(), 1);
        QVERIFY(!diagnostics.storageOldest().isEmpty());
    }

    void changesDebugAndPurgesThroughTheLiveRustService() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise diagnostic controls");
        }
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend(
            {.executable = executable, .directory = directory.path(), .externalEndpoint = {}});
        ctxfab::Diagnostics diagnostics(backend);
        diagnostics.setPageVisible(true);
        QCOMPARE(diagnostics.feedback(), QStringLiteral("Backend session unavailable"));
        backend.start();
        QTRY_VERIFY_WITH_TIMEOUT(diagnostics.ready(), 8000);
        QCOMPARE(diagnostics.healthStatus(), QStringLiteral("Recording"));
        QVERIFY(diagnostics.recording());
        QVERIFY(!diagnostics.debugActive());
        QVERIFY(diagnostics.debugExpiry().isEmpty());
        QVERIFY(!diagnostics.storageBudget().isEmpty());
        QVERIFY(!diagnostics.storageUsed().isEmpty());
        QVERIFY(!diagnostics.storageRetention().isEmpty());
        QVERIFY(!diagnostics.queuedCount().isEmpty());
        QVERIFY(!diagnostics.droppedCount().isEmpty());
        QVERIFY(!diagnostics.purgedCount().isEmpty());
        diagnostics.toggleDebug();
        QTRY_VERIFY(diagnostics.debugActive());
        QVERIFY(!diagnostics.debugExpiry().isEmpty());
        diagnostics.toggleDebug();
        QTRY_VERIFY(!diagnostics.debugActive());
        diagnostics.purgeExpired();
        QTRY_VERIFY(!diagnostics.busy());
        QCOMPARE(diagnostics.feedback(), QStringLiteral("Expired records purged."));
        diagnostics.setPageVisible(false);
        backend.restart();
        QTRY_VERIFY_WITH_TIMEOUT(backend.state() == ctxfab::BackendSupervisor::State::Available,
                                 8000);
        QVERIFY(!diagnostics.ready());
        diagnostics.setPageVisible(true);
        QTRY_VERIFY(diagnostics.ready());
    }

    void loadsPagedSpansAndLogsAndReturnsFromRelatedWork() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        const auto route = "/v1/telemetry/traces/" + TraceA.toLatin1();
        const auto olderRoute = route + "?beforeId=" + QByteArray::number(Cursor);
        const auto joinedRoute = "/v1/telemetry/traces/" + TraceB.toLatin1();
        auto child = sample(Cursor + 1, TraceA, "provider_request", Child);
        child.insert("parentSpanId", Parent);
        auto recent = page({child}, {sample(Cursor, TraceA, "observed", Child)});
        recent.insert("nextBeforeId", Cursor);
        fixture.replies.insert(route, {{.body = recent}});

        auto parent = sample(1, TraceA, "api_request", Parent);
        parent.insert("workTraceId", TraceB);
        parent.insert("httpMethod", "POST");
        parent.insert("httpRoute", "/v1/example");
        parent.insert("httpStatus", 202);
        parent.insert("value", 1250);
        parent.insert("startedAtUnixMicros", 20'000'002);
        parent.insert("spanOutcome", "succeeded");
        parent.insert("events",
                      QJsonArray{QJsonObject{{"name", "accepted"}, {"offsetMicros", 125}}});
        const auto older = page({parent}, {sample(2, TraceA, "started", Parent)});
        fixture.replies.insert(olderRoute,
                               {
                                   {.body = {{"code", "busy"}, {"message", "Try again."}},
                                    .status = "503 Unavailable"},
                                   {.body = older},
                               });
        fixture.replies.insert(joinedRoute,
                               {
                                   {.body = page({}, {})},
                                   {.body = page({sample(7, TraceB, "work_finished", Child)}, {})},
                               });

        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::TraceInspector inspector(backend);
        inspector.open("invalid", {});
        QVERIFY(!inspector.visible());
        inspector.open(TraceA, {});
        QTRY_VERIFY(!inspector.busy());
        QCOMPARE(fixture.requests.count(route), 2);
        QCOMPARE(inspector.spans().size(), 1);
        QVERIFY(inspector.selected().parentMissing());
        QCOMPARE(inspector.spanLogs().size(), 1);
        QVERIFY(inspector.hasMore());

        inspector.loadMore();
        QTRY_COMPARE(inspector.feedback(), QStringLiteral("Try again."));
        QVERIFY(inspector.hasMore());
        QCOMPARE(inspector.spans().size(), 1);
        inspector.loadMore();
        QTRY_COMPARE(inspector.spans().size(), 2);
        QVERIFY(!inspector.hasMore());
        QCOMPARE(fixture.requests.count(olderRoute), 2);
        QCOMPARE(inspector.logs().size(), 2);
        QCOMPARE(inspector.selected().title(), QStringLiteral("POST /v1/example"));
        QCOMPARE(inspector.selected().httpStatus(), QStringLiteral("202"));
        QCOMPARE(inspector.selected().outcome(), QStringLiteral("Succeeded"));
        QVERIFY(inspector.selected().duration().endsWith(" ms"));
        QCOMPARE(inspector.selected().events(), QStringList{"accepted · 125 µs"});
        QVERIFY(inspector.selected().timestamp().endsWith(".000002"));
        QVERIFY(inspector.selected().hasStart());
        QCOMPARE(inspector.spanLogs().constFirst().name(), QStringLiteral("started"));
        QCOMPARE(inspector.spans().at(1).depth(), 1);
        QVERIFY(!inspector.spans().at(1).parentMissing());

        inspector.followWorkTrace();
        QTRY_COMPARE(inspector.selected().name(), QStringLiteral("work_finished"));
        QCOMPARE(inspector.traceId(), TraceB);
        QVERIFY(inspector.canGoBack());
        inspector.goBack();
        QTRY_COMPARE(inspector.selected().spanId(), Parent);
        QCOMPARE(inspector.traceId(), TraceA);
        QVERIFY(!inspector.canGoBack());
        QCOMPARE(fixture.requests.count(olderRoute), 3);
        inspector.reload();
        QTRY_COMPARE(fixture.requests.count(olderRoute), 4);
        QTRY_COMPARE(inspector.selected().spanId(), Parent);
        inspector.close();
        QVERIFY(!inspector.visible() && inspector.logs().isEmpty());
    }
};

QTEST_GUILESS_MAIN(TraceInspectorTests)
#include "TraceInspectorTests.moc"
