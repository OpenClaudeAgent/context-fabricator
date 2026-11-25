#include "BackendSupervisor.h"
#include "GitHubActionRun.h"
#include "GitHubActionWorkflow.h"
#include "GitHubActionsActivity.h"
#include "GitHubConnector.h"
#include "GitHubGraphBrowser.h"
#include "GitHubGraphHit.h"
#include "GitHubSelection.h"
#include "GitHubState.h"
#include "TraceInspector.h"

#include <QDesktopServices>
#include <QFile>
#include <QJsonObject>
#include <QLocale>
#include <QProcess>
#include <QTemporaryDir>
#include <QUrlQuery>
#include <QtTest>

#include <algorithm>

namespace {

using Connector = ctxfab::GitHubConnector;
using Activity = ctxfab::GitHubActionsActivity;
using Graph = ctxfab::GitHubGraphBrowser;
constexpr int AllFeatures =
    Connector::BranchFiles | Connector::CommitHistory | Connector::Actions | Connector::CodeGraph;
const auto Primary = QStringLiteral("github_pat_fixture_primary");
const auto Replacement = QStringLiteral("github_pat_fixture_replacement");

class LinkRecorder : public QObject {
    Q_OBJECT

  public:
    QList<QUrl> urls;

  public slots:
    void capture(const QUrl &url) { urls.append(url); }
};

class Process : public QProcess {
  public:
    ~Process() override {
        if (state() != NotRunning) {
            terminate();
            if (!waitForFinished(2500)) {
                kill();
                waitForFinished(1000);
            }
        }
    }
};

class Fixture {
  public:
    void start(const QString &executable, bool scheduled = false) {
        QVERIFY(directory.isValid());
        origin.start("python3", {"-B", CTXFAB_GITHUB_FIXTURE, directory.path()});
        QVERIFY(origin.waitForStarted());
        QFile port(directory.filePath("port"));
        QTRY_VERIFY_WITH_TIMEOUT(port.exists(), 8000);
        QVERIFY(port.open(QIODevice::ReadOnly));
        const auto number = QString::fromLatin1(port.readAll()).trimmed();
        QVERIFY(number.toInt() > 0);
        const QString mode = scheduled ? "--github-scheduled-fixture" : "--github-fixture";
        server.start(executable, {directory.filePath("profile"), mode, number});
        QVERIFY(server.waitForStarted());
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(endpoint()), 8000);
    }

    [[nodiscard]] QString endpoint() const { return directory.filePath("profile/endpoint.json"); }
    [[nodiscard]] QString marker(const QString &name) const { return directory.filePath(name); }

  private:
    QTemporaryDir directory;
    Process origin;
    Process server;
};

} // namespace

class GitHubTests : public QObject {
    Q_OBJECT

  private slots:
    void keepsTheContractRevisionRange() {
        constexpr qint64 revision = 4294967294;
        ctxfab::openapi::GitHubState state;
        state.fromJsonObject(QJsonObject{{"revision", revision}});
        QCOMPARE(state.getRevision(), revision);
        ctxfab::openapi::GitHubSelection selection;
        selection.setRevision(state.getRevision());
        QCOMPARE(selection.asJsonObject().value("revision").toInteger(), revision);
    }

    void presentsTheTrackedBranchAndEachAccessWarning() {
        using Access = ctxfab::openapi::GitHubAccess::eGitHubAccess;
        ctxfab::openapi::GitHubRepository repository;
        repository.setId(9007199254740997LL);
        repository.setFullName("fixture/alpha");
        repository.setDefaultBranch("main");
        repository.setRPrivate(true);
        const auto candidate = ctxfab::GitHubRepositoryEntry(repository);
        QCOMPARE(candidate.id(), 9007199254740997LL);
        QCOMPARE(candidate.branch(), QStringLiteral("main"));
        QVERIFY(candidate.privateRepository());
        QVERIFY(candidate.accessWarning().isEmpty());

        ctxfab::openapi::GitHubConfiguredRepository configured;
        configured.setRepository(repository);
        configured.setBranch("release/v1");
        const auto warning = [&](bool verified, Access contents, Access actions) {
            ctxfab::openapi::GitHubAccess contentsAccess;
            ctxfab::openapi::GitHubAccess actionsAccess;
            contentsAccess.setValue(contents);
            actionsAccess.setValue(actions);
            configured.setVerified(verified);
            configured.setContents(contentsAccess);
            configured.setActions(actionsAccess);
            return ctxfab::GitHubRepositoryEntry(configured).accessWarning();
        };
        QCOMPARE(warning(false, Access::UNAVAILABLE, Access::UNAVAILABLE), "Verify access");
        QCOMPARE(warning(true, Access::UNAVAILABLE, Access::UNAVAILABLE), "Access unverified");
        QCOMPARE(warning(true, Access::AVAILABLE, Access::UNAVAILABLE),
                 "Actions access unverified");
        QCOMPARE(warning(true, Access::UNAVAILABLE, Access::UNCHECKED),
                 "Contents access unverified");
        QVERIFY(warning(true, Access::AVAILABLE, Access::UNCHECKED).isEmpty());
        QCOMPARE(ctxfab::GitHubRepositoryEntry(configured).branch(), QStringLiteral("release/v1"));
    }

    void keepsRepositoryIdentityAcrossMetadataChanges() {
        ctxfab::GitHubRepositoryList model;
        ctxfab::openapi::GitHubRepository repository;
        repository.setId(9007199254740997LL);
        repository.setFullName("fixture/alpha");
        repository.setDefaultBranch("main");
        const QSignalSpy resets(&model, &QAbstractItemModel::modelReset);
        const QSignalSpy changes(&model, &QAbstractItemModel::dataChanged);
        const QSignalSpy names(&model, &ctxfab::GitHubRepositoryList::namesChanged);
        model.replace({ctxfab::GitHubRepositoryEntry(repository)});
        QCOMPARE(resets.count(), 1);
        QCOMPARE(model.count(), 1);
        const auto index = model.index(0);
        QVERIFY(!model.data(index, Qt::DisplayRole).isValid());
        const int entryRole = model.roleNames().key("entry");
        QCOMPARE(model.data(index, entryRole).value<ctxfab::GitHubRepositoryEntry>().name(),
                 QStringLiteral("fixture/alpha"));

        repository.setRPrivate(true);
        model.replace({ctxfab::GitHubRepositoryEntry(repository)});
        QVERIFY(model.entryAt(0).privateRepository());
        QCOMPARE(changes.count(), 1);
        QCOMPARE(names.count(), 1);
        repository.setFullName("fixture/renamed");
        model.replace({ctxfab::GitHubRepositoryEntry(repository)});
        QCOMPARE(model.names(), QStringList{"fixture/renamed"});
        QCOMPARE(names.count(), 2);
        QCOMPARE(resets.count(), 1);

        repository.setId(9007199254740998LL);
        model.replace({ctxfab::GitHubRepositoryEntry(repository)});
        QCOMPARE(resets.count(), 2);
        QVERIFY(!model.data({}, entryRole).isValid());
    }

    void acceptsRequiredNullableActionsFields() {
        QJsonObject workflow{{"id", 21},
                             {"name", "Dependabot Updates"},
                             {"path", "dynamic/dependabot/dependabot-updates"},
                             {"state", "active"},
                             {"definitionBlob", QJsonValue::Null},
                             {"definition", QJsonValue::Null}};
        ctxfab::openapi::GitHubActionWorkflow definition;
        definition.fromJsonObject(workflow);
        QVERIFY(definition.isValid());
        QVERIFY(!definition.is_definition_Set());
        QCOMPARE(definition.asJsonObject(), workflow);
        workflow.remove("definition");
        definition.fromJsonObject(workflow);
        QVERIFY(!definition.isValid());

        const QJsonObject queued{
            {"id", 30},
            {"attempt", 1},
            {"workflowId", 21},
            {"event", "push"},
            {"headSha", QString(40, 'a')},
            {"headBranch", QJsonValue::Null},
            {"headRepositoryId", QJsonValue::Null},
            {"commitProvenance", "forkOrUnknownReportedHead"},
            {"createdAt", "2026-01-01T00:00:00Z"},
            {"updatedAt", "2026-01-01T00:00:00Z"},
            {"status", "queued"},
            {"conclusion", QJsonValue::Null},
        };
        ctxfab::openapi::GitHubActionRun attempt;
        auto completed = queued;
        completed.insert("conclusion", "success");
        attempt.fromJsonObject(completed);
        QCOMPARE(attempt.getConclusion(), QStringLiteral("success"));
        attempt.fromJsonObject(queued);
        QVERIFY(attempt.isValid());
        QVERIFY(!attempt.is_conclusion_Set());
        QVERIFY(attempt.getConclusion().isEmpty());
        QVERIFY(attempt.asJsonObject().value("headRepositoryId").isNull());
        auto large = queued;
        large.insert("id", 9'007'199'254'740'995LL);
        large.insert("attempt", 4'294'967'295LL);
        attempt.fromJsonObject(large);
        const ctxfab::GitHubActionsRun row(attempt, {}, "fixture/alpha");
        QCOMPARE(row.url(), QUrl("https://github.com/fixture/alpha/actions/runs/9007199254740995/"
                                 "attempts/4294967295"));
        QVERIFY(ctxfab::GitHubActionsRun(attempt, {}, "fixture/..").url().isEmpty());
    }

    void readsFiltersAndRefreshesActionsAttempts() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise Actions");
        }
        Fixture fixture;
        fixture.start(executable);
        QVERIFY(!QTest::currentTestFailed());
        for (const auto *name :
             {"actions-fixture", "actions-dynamic-workflow", "collect-history"}) {
            QFile flag(fixture.marker(QString::fromLatin1(name)));
            QVERIFY(flag.open(QIODevice::WriteOnly));
        }
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        Connector connector(backend);
        Activity activity(backend, connector);
        connector.setCollectionVisible(true);
        QTRY_VERIFY(connector.ready());
        QVERIFY(!activity.active());
        connector.authorize(Primary);
        QTRY_COMPARE(connector.credential(), Connector::Connected);
        connector.addRepository({});
        connector.saveRepository("fixture/alpha", "main", 60, Connector::Actions);
        QTRY_COMPARE(connector.repositoryList()->names(), QStringList{"fixture/alpha"});
        connector.editRepository(0);
        QTRY_COMPARE(activity.availability(), Activity::Ready);
        QVERIFY(!activity.hasSnapshot());
        activity.show();
        QTRY_VERIFY(!activity.busy());
        QVERIFY(activity.visible());
        activity.refresh();
        QTRY_COMPARE_WITH_TIMEOUT(activity.runs().size(), 1, 12000);
        QVERIFY(activity.hasSnapshot());
        QVERIFY(!activity.nextCheck().isEmpty());
        QCOMPARE(activity.workflows(), (QStringList{"All Workflows", "CI", "Dependabot Updates"}));
        const auto first = activity.runs().constFirst();
        QCOMPARE(first.summary(), QStringLiteral("Run 30 · attempt 1 · push · aaaaaaaaaa"));
        QCOMPARE(first.resultLabel(), QStringLiteral("Queued"));
        QCOMPARE(first.url(), QUrl("https://github.com/fixture/alpha/actions/runs/30/attempts/1"));
        activity.selectWorkflow(2);
        QCOMPARE(activity.workflowIndex(), 2);
        QVERIFY(activity.runs().isEmpty());

        QFile rerun(fixture.marker("actions-rerun"));
        QVERIFY(rerun.open(QIODevice::WriteOnly));
        rerun.close();
        QTRY_VERIFY(!connector.busy());
        activity.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!activity.busy() && !connector.busy(), 12000);
        QCOMPARE(activity.workflowIndex(), 2);
        QVERIFY(activity.runs().isEmpty());
        activity.selectWorkflow(1);
        const auto attempts = activity.runs();
        QCOMPARE(attempts.size(), 2);
        QCOMPARE(attempts.at(0).attempt(), 2);
        QCOMPARE(attempts.at(0).resultLabel(), QStringLiteral("Success"));
        QCOMPARE(attempts.at(0).url(),
                 QUrl("https://github.com/fixture/alpha/actions/runs/30/attempts/2"));
        QCOMPARE(attempts.at(1).attempt(), 1);
        const auto next = activity.nextCheck();
        QFile badBlob(fixture.marker("actions-bad-blob"));
        QVERIFY(badBlob.open(QIODevice::WriteOnly));
        badBlob.close();
        activity.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!activity.busy() && !connector.busy(), 12000);
        QVERIFY(!activity.feedback().isEmpty());
        QVERIFY(activity.feedback() != "Workflow runs refreshed.");
        QCOMPARE(activity.runs().size(), 2);
        QCOMPARE(activity.nextCheck(), next);

        activity.close();
        QVERIFY(!activity.visible());
        connector.closeDraft();
        QVERIFY(!activity.active() && activity.runs().isEmpty());
        connector.disconnectAccount();
        QTRY_COMPARE(connector.credential(), Connector::Disconnected);
        connector.editRepository(0);
        QTRY_COMPARE(activity.availability(), Activity::ReconnectRequired);
        QCOMPARE(activity.runs().size(), 2);
        activity.refresh();
        QVERIFY(!activity.busy());

        connector.closeDraft();
        connector.authorize(Primary);
        QTRY_COMPARE(connector.credential(), Connector::Connected);
        connector.addRepository({});
        connector.saveRepository("fixture/beta", "trunk", 60, Connector::Actions);
        QTRY_COMPARE(connector.repositoryList()->names().size(), 2);
        connector.editRepository(1);
        QTRY_COMPARE(activity.availability(), Activity::AccessRequired);
        QVERIFY(!activity.hasSnapshot());
        connector.authorize(Replacement);
        QTRY_VERIFY(!connector.busy());
        connector.verifyAccess();
        QTRY_COMPARE(activity.availability(), Activity::Ready);
        QVERIFY(connector.draftNeedsReview());
        QCOMPARE(activity.branch(), QStringLiteral("trunk"));
    }

    void refreshesHistoryAndOpensThePublishedCollectionTrace() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise history and collection traces");
        }
        Fixture fixture;
        fixture.start(executable);
        QVERIFY(!QTest::currentTestFailed());
        QFile lineage(fixture.marker("collect-history"));
        QVERIFY(lineage.open(QIODevice::WriteOnly));
        lineage.close();

        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        Connector connector(backend);
        ctxfab::TraceInspector inspector(backend);
        connector.setCollectionVisible(true);
        QTRY_VERIFY(connector.ready());
        connector.authorize(Primary);
        QTRY_COMPARE(connector.credential(), Connector::Connected);
        connector.addRepository({});
        connector.saveRepository("fixture/alpha", "main", 15, Connector::CommitHistory);
        QTRY_COMPARE(connector.repositoryList()->names(), QStringList{"fixture/alpha"});
        connector.editRepository(0);
        QVERIFY(connector.canRefreshHistory());
        QVERIFY(!connector.canRefreshBranch());
        QVERIFY(connector.historyTraces().isEmpty());

        connector.refreshHistory();
        QTRY_COMPARE_WITH_TIMEOUT(connector.feedback(), QStringLiteral("Code history refreshed."),
                                  12000);
        const auto traces = connector.historyTraces();
        QCOMPARE(traces.size(), 1);
        QCOMPARE(traces.constFirst().label(), QStringLiteral("View collection trace"));
        const auto trace = traces.constFirst();
        inspector.open(trace.traceId(), trace.spanId());
        QTRY_COMPARE(inspector.selected().name(), QStringLiteral("github_history_collection"));
        const auto spans = inspector.spans();
        const auto provider = std::ranges::find_if(spans, [](const auto &record) {
            return record.name() == "github_provider_request" &&
                   record.httpRequest() == "GET /repos/{owner}/{repo}";
        });
        QVERIFY(provider != spans.end());
        inspector.selectSpan(static_cast<int>(std::distance(spans.begin(), provider)));
        QCOMPARE(inspector.selected().httpStatus(), QStringLiteral("200"));
        QVERIFY(inspector.selected().depth() > 0);
        inspector.close();
        QVERIFY(!inspector.visible() && inspector.spans().isEmpty());

        connector.purgeRepositoryData();
        QTRY_VERIFY(!connector.busy() && connector.draftDataPaused());
        QVERIFY(connector.historyTraces().isEmpty());
        QVERIFY(connector.canRefreshHistory());
        connector.refreshHistory();
        QTRY_COMPARE_WITH_TIMEOUT(connector.feedback(), QStringLiteral("Code history refreshed."),
                                  12000);
        QTRY_VERIFY(connector.draftDataKnown() && !connector.draftDataPaused());
        QCOMPARE(connector.historyTraces().size(), 1);
        QVERIFY(connector.historyTraces().constFirst().traceId() != trace.traceId());

        const auto published = connector.historyTraces().constFirst();
        QFile deny(fixture.marker("deny-contents"));
        QVERIFY(deny.open(QIODevice::WriteOnly));
        deny.close();
        connector.refreshHistory();
        QTRY_VERIFY_WITH_TIMEOUT(!connector.busy(), 12000);
        QVERIFY(connector.feedback() != "Code history refreshed.");
        const auto afterFailure = connector.historyTraces();
        QCOMPARE(afterFailure.size(), 2);
        QCOMPARE(afterFailure.at(0).label(), QStringLiteral("View latest attempt trace"));
        QCOMPARE(afterFailure.at(1).label(), QStringLiteral("View published snapshot trace"));
        QCOMPARE(afterFailure.at(1).traceId(), published.traceId());
        QVERIFY(afterFailure.at(0).traceId() != published.traceId());
    }

    void keepsTheConnectorInactiveWithoutASession() {
        const QTemporaryDir directory;
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = directory.filePath("missing")});
        Connector connector(backend);
        connector.setCollectionVisible(true);
        QCOMPARE(connector.credential(), Connector::Unknown);
        QCOMPARE(connector.feedback(), QStringLiteral("Backend session unavailable"));
        connector.authorize(Primary);
        connector.addRepository({});
        connector.discover();
        connector.openTokenPage(true);
        QVERIFY(!connector.busy() && !connector.ready() && !connector.draftOpen());
        QVERIFY(connector.tokenUrl(true).isEmpty());
        QCOMPARE(connector.minimumInterval(), 15);
        QCOMPARE(connector.maximumInterval(), 1440);
    }

    void reloadsAfterReplacingTheManagedSession() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the GitHub connector");
        }
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend({.executable = executable,
                                           .directory = directory.filePath("profile"),
                                           .externalEndpoint = {}});
        Connector connector(backend);
        backend.start();
        QTRY_VERIFY(backend.session().has_value());
        QVERIFY(!connector.ready());
        connector.setCollectionVisible(true);
        QTRY_VERIFY(connector.ready());
        const auto before = backend.session()->processId;
        backend.restart();
        QVERIFY(!connector.ready());
        QTRY_VERIFY(connector.ready());
        QVERIFY(backend.session()->processId != before);
        QCOMPARE(connector.credential(), Connector::Disconnected);
    }

    void describesCollectionStates_data() {
        QTest::addColumn<QString>("phase");
        QTest::addColumn<QString>("label");
        QTest::addColumn<int>("delay");
        QTest::newRow("disabled") << QString("disabled") << QString("Collection off") << -1;
        QTest::newRow("credentials")
            << QString("credentials") << QString("Read access required") << -1;
        QTest::newRow("queued") << QString("queued") << QString("Queued") << 2000;
        QTest::newRow("running") << QString("running") << QString("Preparing index") << 2000;
        QTest::newRow("succeeded") << QString("succeeded") << QString("Up to date") << 7000;
        QTest::newRow("retry") << QString("retry") << QString("Retry scheduled") << 7000;
        QTest::newRow("throttled") << QString("throttled") << QString("Waiting for GitHub") << 7000;
        QTest::newRow("failed") << QString("failed") << QString("Refresh failed") << -1;
    }

    void describesCollectionStates() {
        QFETCH(QString, phase);
        QFETCH(QString, label);
        QFETCH(int, delay);
        const bool waitingForSchedule = delay == 7000;
        const bool running = phase == "running";
        ctxfab::openapi::GitHubCodeCollection model;
        model.fromJsonObject({{"phase", phase},
                              {"canRefresh", true},
                              {"lastCheckedAtMs", 0},
                              {"publishedFiles", 0},
                              {"error", "Collection failed."}});
        if (waitingForSchedule) {
            model.setNextAtMs(10000);
        }
        if (running) {
            ctxfab::openapi::GitHubSourceProgress pending;
            pending.fromJsonObject(
                {{"phase", "manifestComplete"}, {"commit", QString(40, 'b')}, {"stagedFiles", 0}});
            model.setPending(pending);
        }
        const ctxfab::GitHubCollection collection(model, true);
        QVERIFY(collection.available());
        QVERIFY(collection.graphOnly());
        QCOMPARE(collection.status(), label);
        QCOMPARE(collection.lastChecked().isEmpty(), running);
        QCOMPARE(collection.nextCheck().isEmpty(), !waitingForSchedule);
        QCOMPARE(collection.error().isEmpty(), phase != "retry" && phase != "failed");
        QCOMPARE(collection.pendingFiles(), running ? 0 : -1);
        QCOMPARE(collection.publishedFiles(), 0);
        QCOMPARE(collection.pollDelay(5000), delay);
        if (waitingForSchedule) {
            QCOMPARE(collection.pollDelay(10000), 10000);
        }
    }

    void keepsExactStorageAndTrafficCounters() {
        constexpr qint64 large = 9'007'199'254'740'995;
        ctxfab::openapi::GitHubTraffic traffic;
        traffic.fromJsonObject({{"attempts", large},
                                {"failures", 2},
                                {"pending", 3},
                                {"uncertain", 4},
                                {"queued", 5},
                                {"concurrency", 1},
                                {"effectiveRequestsPerMinute", 75},
                                {"maximumConfigurableRequestsPerMinute", 60}});
        ctxfab::openapi::GitHubStorageBudget budget;
        budget.fromJsonObject({{"revision", 0}, {"limitGb", 10}, {"bytes", large}});
        const ctxfab::GitHubUsage usage(traffic, budget);
        const QLocale locale;
        QVERIFY(usage.hasTraffic());
        QCOMPARE(usage.effectiveRate(), 75);
        QCOMPARE(usage.requestedRate(), 60);
        QCOMPARE(usage.storageLimit(), 10);
        QCOMPARE(usage.bytes(), large);
        QCOMPARE(usage.attempts(), locale.toString(large));
        QCOMPARE(usage.failures(), locale.toString(2));
        QCOMPARE(usage.inFlight(), locale.toString(3));
        QCOMPARE(usage.uncertain(), locale.toString(4));
        QCOMPARE(usage.queued(), locale.toString(5));
        QCOMPARE(usage.concurrency(), locale.toString(1));
        QVERIFY(usage.retainedPayload().contains(locale.toString(large)));
        QVERIFY(usage.cooldown().isEmpty());
        traffic.setCooldownUntilMs(86'400'000);
        QVERIFY(!ctxfab::GitHubUsage(traffic, budget).cooldown().isEmpty());
    }

    void linksToThePinnedSourceLocation() {
        const QString revision(40, 'a');
        const auto base = "https://github.com/fixture/alpha/blob/" + revision;
        const auto location = QJsonObject{
            {"path", QStringLiteral("src/naïve.rs")},
            {"fileUrl", base + "/src/na%C3%AFve.rs"},
            {"blob", QString(40, 'b')},
            {"startByte", 20},
            {"endByte", 42},
            {"startLine", 2},
            {"endLine", 3},
        };
        ctxfab::openapi::GitHubGraphHit hit;
        hit.fromJsonObject({{"name", "example"},
                            {"role", "call"},
                            {"resolution", "resolved"},
                            {"site", location}});
        const ctxfab::GitHubGraphOccurrence valid(hit, "fixture/alpha", revision);
        QCOMPARE(valid.sourceText(), QStringLiteral("src/naïve.rs#L2-L3"));
        QCOMPARE(valid.sourceUrl(), QUrl(base + "/src/na%C3%AFve.rs#L2-L3"));

        auto unsafe = hit.getSite();
        unsafe.setFileUrl(base + "/src%2Fna%C3%AFve.rs");
        hit.setSite(unsafe);
        QVERIFY(
            ctxfab::GitHubGraphOccurrence(hit, "fixture/alpha", revision).sourceUrl().isEmpty());
        unsafe.setFileUrl(base + "/src/na%C3%AFve.rs");
        unsafe.setEndLine(1);
        hit.setSite(unsafe);
        QVERIFY(
            ctxfab::GitHubGraphOccurrence(hit, "fixture/alpha", revision).sourceUrl().isEmpty());
    }

    void browsesThePublishedGraphAndKeepsItAfterDisconnecting() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the GitHub graph");
        }
        Fixture fixture;
        fixture.start(executable, true);
        QVERIFY(!QTest::currentTestFailed());
        for (const auto *name : {"collect-graph", "second-graph-root"}) {
            QFile collect(fixture.marker(QString::fromLatin1(name)));
            QVERIFY(collect.open(QIODevice::WriteOnly));
        }

        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        Connector connector(backend);
        Graph graph(backend);
        connect(&connector, &Connector::graphRequested, &graph, &Graph::open);
        connector.setCollectionVisible(true);
        QTRY_VERIFY(connector.ready());
        QVERIFY(!connector.canBrowseGraph());

        connector.authorize(Primary);
        QTRY_COMPARE(connector.credential(), Connector::Connected);
        connector.addRepository({});
        connector.saveRepository("fixture/alpha", "main", 60, Connector::BranchFiles);
        QTRY_COMPARE(connector.repositoryList()->names(), QStringList{"fixture/alpha"});
        connector.editRepository(0);
        QVERIFY(!connector.canBrowseGraph());
        graph.open(41, "fixture/alpha", "main");
        QTRY_VERIFY(!graph.busy());
        QCOMPARE(graph.access(), Graph::Unavailable);
        QCOMPARE(graph.coverage(), Graph::NoSnapshot);
        QVERIFY(graph.feedback().isEmpty());
        graph.close();

        connector.saveRepository("fixture/alpha", "main", 60, Connector::CodeGraph);
        QTRY_VERIFY(!connector.draftOpen());
        connector.editRepository(0);
        QVERIFY(connector.canBrowseGraph());
        QTRY_COMPARE_WITH_TIMEOUT(connector.draftCollection().commit(), QString(40, 'a'), 12000);

        connector.browseGraph();
        QTRY_COMPARE(graph.coverage(), Graph::Published);
        QCOMPARE(graph.branch(), QStringLiteral("main"));
        QCOMPARE(graph.access(), Graph::Available);
        QCOMPARE(graph.roots(), (QStringList{"src/lib.rs", "src/main.rs"}));
        QCOMPARE(graph.rootIndex(), 0);
        QVERIFY(graph.rootSummary().contains(QStringLiteral("2 reachable files")));
        graph.lookup("   ");
        QVERIFY(!graph.busy());
        graph.lookup("  crate::target::graph_example  ");
        QTRY_COMPARE(graph.hits().size(), 2);
        QCOMPARE(graph.searchedRoot(), QStringLiteral("src/lib.rs"));
        QCOMPARE(graph.searchedSymbol(), QStringLiteral("crate::target::graph_example"));
        const auto hits = graph.hits();
        const auto call =
            std::ranges::find_if(hits, [](const auto &hit) { return hit.role() == "call"; });
        QVERIFY(call != hits.end());
        QCOMPARE(call->resolution(), QStringLiteral("resolved"));
        const auto base = "https://github.com/fixture/alpha/blob/" + QString(40, 'a') + "/src/";
        QCOMPARE(call->sourceUrl(), QUrl(base + "lib.rs#L2"));
        QCOMPARE(call->targetUrl(), QUrl(base + "target.rs#L1"));
        graph.lookup("invalid symbol");
        QTRY_VERIFY(!graph.feedback().isEmpty());
        QCOMPARE(graph.hits().size(), 2);

        graph.selectRoot(1);
        QCOMPARE(graph.rootIndex(), 1);
        graph.lookup("graph_example");
        QTRY_COMPARE(graph.searchedRoot(), QStringLiteral("src/main.rs"));
        QVERIFY(graph.hits().isEmpty());
        graph.lookup("local_only");
        QTRY_COMPARE(graph.hits().size(), 1);
        QCOMPARE(graph.hits().constFirst().role(), QStringLiteral("definition"));
        graph.selectRoot(0);

        graph.lookup("missing_symbol");
        QTRY_COMPARE(graph.searchedSymbol(), QStringLiteral("missing_symbol"));
        QVERIFY(graph.hits().isEmpty());
        graph.lookup("graph_example");
        QTRY_COMPARE(graph.hits().size(), 2);

        QFile advance(fixture.marker("advance-code"));
        QVERIFY(advance.open(QIODevice::WriteOnly));
        advance.close();
        connector.refreshGraphSource("release/v1");
        QVERIFY(!connector.busy());
        connector.refreshGraphSource(graph.branch());
        QTRY_COMPARE_WITH_TIMEOUT(connector.draftCollection().commit(), QString(40, 'b'), 12000);
        QCOMPARE(graph.commit(), QString(10, 'a'));
        graph.reload();
        QTRY_COMPARE(graph.commit(), QString(10, 'b'));
        QCOMPARE(graph.hits().size(), 1);
        QCOMPARE(graph.hits().constFirst().resolution(), QStringLiteral("unresolved"));
        QVERIFY(graph.hits().constFirst().targetUrl().isEmpty());

        graph.close();
        QVERIFY(!graph.visible() && graph.hits().isEmpty());
        connector.closeDraft();
        connector.disconnectAccount();
        QTRY_COMPARE(connector.credential(), Connector::Disconnected);
        connector.editRepository(0);
        QVERIFY(connector.canBrowseGraph());
        connector.browseGraph();
        QTRY_COMPARE(graph.access(), Graph::Retained);
        QCOMPARE(graph.commit(), QString(10, 'b'));
        graph.lookup("graph_example");
        QTRY_COMPARE(graph.hits().size(), 1);

        graph.open(41, "fixture/alpha", "release/v1");
        QVERIFY(graph.hits().isEmpty());
        QTRY_VERIFY(!graph.busy());
        QCOMPARE(graph.branch(), QStringLiteral("release/v1"));
        QCOMPARE(graph.coverage(), Graph::NoSnapshot);
        QVERIFY(graph.feedback().isEmpty());
    }

    void refreshesAndObservesBranchFilesWithoutReplacingTheLastPublication() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the GitHub scheduler");
        }
        Fixture fixture;
        fixture.start(executable, true);
        QVERIFY(!QTest::currentTestFailed());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        Connector connector(backend);
        connector.setCollectionVisible(true);
        QTRY_VERIFY(connector.ready());
        connector.authorize(Primary);
        QTRY_COMPARE(connector.credential(), Connector::Connected);
        connector.addRepository({});
        connector.saveRepository("fixture/alpha", "main", 60, Connector::BranchFiles);
        QTRY_COMPARE(connector.repositoryList()->names(), QStringList{"fixture/alpha"});
        connector.editRepository(0);
        const QString original(40, 'a');
        const QString next(40, 'b');
        QTRY_COMPARE_WITH_TIMEOUT(connector.draftCollection().commit(), original, 12000);
        QCOMPARE(connector.draftCollection().publishedFiles(), 2);
        QVERIFY(connector.draftCollection().canRefresh());
        auto *repositories = connector.repositoryList();
        QCOMPARE(repositories->entryAt(0).collectionStatus(), QStringLiteral("Up to date"));
        const QSignalSpy repositoryResets(repositories, &QAbstractItemModel::modelReset);
        const QSignalSpy repositoryChanges(repositories, &QAbstractItemModel::dataChanged);

        QFile hold(fixture.marker("hold-code-blob"));
        QVERIFY(hold.open(QIODevice::WriteOnly));
        hold.close();
        QFile advance(fixture.marker("advance-code"));
        QVERIFY(advance.open(QIODevice::WriteOnly));
        advance.close();
        connector.refreshBranch();
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(fixture.marker("held-code-blob")), 10000);
        QTRY_COMPARE_WITH_TIMEOUT(connector.draftCollection().pendingCommit(), next, 4500);
        QCOMPARE(connector.draftCollection().pendingFiles(), 1);
        QCOMPARE(connector.draftCollection().commit(), original);
        QCOMPARE(connector.draftCollection().publishedFiles(), 2);
        QVERIFY(connector.draftCollection().running());
        QCOMPARE(connector.draftCollection().status(), QString("Syncing files"));
        QVERIFY(connector.feedback().isEmpty());

        connector.setCollectionVisible(false);
        QVERIFY(hold.remove());
        Connector observer(backend);
        observer.setCollectionVisible(true);
        QTRY_COMPARE(observer.repositoryList()->names(), QStringList{"fixture/alpha"});
        observer.editRepository(0);
        QTRY_COMPARE_WITH_TIMEOUT(observer.draftCollection().commit(), next, 10000);
        QCOMPARE(connector.draftCollection().commit(), original);
        connector.setCollectionVisible(true);
        QTRY_COMPARE(connector.draftCollection().commit(), next);
        QCOMPARE(connector.draftCollection().pendingFiles(), -1);
        QCOMPARE(connector.draftCollection().publishedFiles(), 2);
        QVERIFY(!connector.draftCollection().running());
        QVERIFY(!connector.draftNeedsReview());
        QVERIFY(repositoryResets.isEmpty());
        QVERIFY(!repositoryChanges.isEmpty());
    }

    void changesTheQuotaAndPurgesOnlyTheSelectedRepository() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise GitHub storage");
        }
        Fixture fixture;
        fixture.start(executable, true);
        QVERIFY(!QTest::currentTestFailed());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        Connector connector(backend);
        connector.setCollectionVisible(true);
        QTRY_VERIFY(connector.ready());
        connector.authorize(Primary);
        QTRY_COMPARE(connector.credential(), Connector::Connected);
        connector.addRepository({});
        connector.saveRepository("fixture/alpha", "main", 60, Connector::BranchFiles);
        QTRY_COMPARE(connector.repositoryList()->names(), QStringList{"fixture/alpha"});
        connector.addRepository({});
        connector.saveRepository("fixture/beta", "trunk", 60, Connector::BranchFiles);
        QTRY_COMPARE(connector.repositoryList()->names().size(), 2);
        connector.editRepository(1);
        QTRY_VERIFY_WITH_TIMEOUT(!connector.draftCollection().commit().isEmpty(), 12000);
        const auto beta = connector.draftCollection().commit();
        connector.editRepository(0);
        QTRY_VERIFY_WITH_TIMEOUT(!connector.draftCollection().commit().isEmpty(), 12000);
        const auto alpha = connector.draftCollection().commit();
        QTRY_VERIFY(connector.draftDataKnown());
        QVERIFY(!connector.draftDataPaused());
        connector.closeDraft();

        Connector peer(backend);
        connector.loadStorage();
        peer.loadStorage();
        QTRY_COMPARE(connector.usage().storageLimit(), 10);
        QTRY_COMPARE(peer.usage().storageLimit(), 10);
        const auto before = connector.usage().bytes();
        QVERIFY(before > 0);
        connector.setStorageLimit(1);
        QTRY_COMPARE(connector.usage().storageLimit(), 1);
        QCOMPARE(connector.usage().bytes(), before);
        peer.setStorageLimit(2);
        QTRY_VERIFY(!peer.feedback().isEmpty());
        QCOMPARE(peer.usage().storageLimit(), 10);
        peer.loadStorage();
        QTRY_COMPARE(peer.usage().storageLimit(), 1);

        connector.editRepository(0);
        QTRY_VERIFY(connector.draftDataKnown());
        connector.purgeRepositoryData();
        QTRY_VERIFY(connector.draftDataPaused() && !connector.busy());
        QVERIFY(connector.draftCollection().commit().isEmpty());
        QCOMPARE(connector.repositoryList()->names(),
                 (QStringList{"fixture/alpha", "fixture/beta"}));
        QVERIFY(connector.usage().bytes() > 0 && connector.usage().bytes() < before);
        connector.closeDraft();
        connector.editRepository(0);
        QTRY_VERIFY(connector.draftDataKnown() && connector.draftDataPaused());
        QVERIFY(connector.canRefreshBranch());
        connector.refreshBranch();
        QTRY_VERIFY(!connector.draftDataPaused() && !connector.busy());
        QTRY_COMPARE_WITH_TIMEOUT(connector.draftCollection().commit(), alpha, 12000);
        connector.editRepository(1);
        QCOMPARE(connector.draftCollection().commit(), beta);
        QTRY_VERIFY(connector.draftDataKnown());
        QVERIFY(!connector.draftDataPaused());
        connector.closeDraft();

        connector.loadStorage();
        QTRY_COMPARE(connector.usage().bytes(), before);
        QVERIFY(connector.usage().hasTraffic());
        connector.setRequestLimit(42);
        QTRY_COMPARE(connector.usage().requestedRate(), 42);
    }

    void configuresRepositoriesThroughTheRealBackend() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the GitHub connector");
        }
        Fixture fixture;
        fixture.start(executable);
        QVERIFY(!QTest::currentTestFailed());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        Connector first(backend);
        first.setCollectionVisible(true);
        QTRY_VERIFY(first.ready());
        QCOMPARE(first.credential(), Connector::Disconnected);
        QCOMPARE(first.tokenUrl(false).host(), QStringLiteral("github.com"));
        QCOMPARE(QUrlQuery(first.tokenUrl(true)).queryItemValue("actions"), QStringLiteral("read"));
        LinkRecorder recorder;
        QDesktopServices::setUrlHandler("https", &recorder, "capture");
        first.openTokenPage(false);
        first.openTokenPage(true);
        QDesktopServices::unsetUrlHandler("https");
        QCOMPARE(recorder.urls, (QList<QUrl>{first.tokenUrl(false), first.tokenUrl(true)}));

        first.authorize("github_pat_invalid");
        QTRY_VERIFY_WITH_TIMEOUT(!first.busy(), 18000);
        QCOMPARE(first.credential(), Connector::Disconnected);
        QVERIFY(!first.feedback().isEmpty());
        QVERIFY(!first.feedback().contains("github_pat_invalid"));
        first.authorize(Primary);
        QTRY_COMPARE_WITH_TIMEOUT(first.credential(), Connector::Connected, 18000);
        QCOMPARE(first.login(), QStringLiteral("fixture-user"));

        first.discover();
        QTRY_COMPARE_WITH_TIMEOUT(first.candidateList()->names(), QStringList{"fixture/alpha"},
                                  18000);
        QVERIFY(first.moreCandidates());
        first.filterCandidates("BETA");
        QVERIFY(first.candidateList()->names().isEmpty());
        first.loadMoreCandidates();
        QTRY_COMPARE_WITH_TIMEOUT(first.candidateList()->names(), QStringList{"fixture/beta"},
                                  18000);
        QVERIFY(!first.moreCandidates());
        first.filterCandidates({});
        QCOMPARE(first.candidateList()->names(), (QStringList{"fixture/alpha", "fixture/beta"}));
        first.chooseCandidate(-1);
        QVERIFY(!first.draftOpen());

        first.chooseCandidate(0);
        QVERIFY(first.draftOpen() && !first.editing());
        QCOMPARE(first.draftRepository(), QStringLiteral("fixture/alpha"));
        QCOMPARE(first.draftBranch(), QStringLiteral("main"));
        first.saveRepository(first.draftRepository(), first.draftBranch(), 90, AllFeatures);
        QTRY_COMPARE_WITH_TIMEOUT(first.repositoryList()->names(), QStringList{"fixture/alpha"},
                                  18000);
        QVERIFY(!first.draftOpen());
        QCOMPARE(first.candidateList()->names(), QStringList{"fixture/beta"});

        first.addRepository("  https://github.com/fixture/beta.git  ");
        QCOMPARE(first.draftRepository(), QStringLiteral("https://github.com/fixture/beta.git"));
        first.saveRepository(first.draftRepository(), {}, 60, Connector::BranchFiles);
        QTRY_COMPARE_WITH_TIMEOUT(first.repositoryList()->names().size(), 2, 18000);
        QVERIFY(first.candidateList()->names().isEmpty());
        first.editRepository(0);
        QVERIFY(first.editing());
        QVERIFY(QUrlQuery(first.tokenUrl(false)).queryItemValue("target_name").isEmpty());
        const QUrlQuery scopedToken(first.tokenUrl(true, true));
        QCOMPARE(scopedToken.queryItemValue("target_name"), QStringLiteral("fixture"));
        QCOMPARE(scopedToken.queryItemValue("actions"), QStringLiteral("read"));
        QCOMPARE(first.draftFeatures(), AllFeatures);
        QCOMPARE(first.draftInterval(), 90);
        QVERIFY(!first.branchesLoaded());
        first.loadMoreBranches();
        QTRY_COMPARE_WITH_TIMEOUT(first.branches(), (QStringList{"main", "feature/search"}), 18000);
        QVERIFY(first.branchesLoaded());
        QVERIFY(first.moreBranches());
        first.filterBranches(" RELEASE ");
        QVERIFY(first.matchingBranches().isEmpty());
        first.loadMoreBranches();
        QTRY_COMPARE_WITH_TIMEOUT(first.matchingBranches(), QStringList{"release/v1"}, 18000);
        QVERIFY(!first.moreBranches());
        first.verifyAccess();
        QTRY_VERIFY_WITH_TIMEOUT(!first.busy(), 18000);
        QVERIFY(first.draftVerified());
        QVERIFY(!first.draftNeedsReview());

        Connector second(backend);
        second.setCollectionVisible(true);
        QTRY_VERIFY(second.ready());
        second.editRepository(0);
        second.saveRepository(second.draftRepository(), second.draftBranch(), 120, AllFeatures);
        QTRY_VERIFY_WITH_TIMEOUT(!second.draftOpen(), 18000);
        const int edited = AllFeatures ^ Connector::BranchFiles;
        first.saveRepository(first.draftRepository(), "main", 90, edited);
        QTRY_VERIFY_WITH_TIMEOUT(first.draftNeedsReview(), 18000);
        QVERIFY(!first.draftCanReview());
        first.reload();
        QTRY_VERIFY(first.draftCanReview());
        first.reviewDraft(first.draftRepository(), "main", 90, edited);
        QVERIFY(!first.draftNeedsReview());
        QCOMPARE(first.draftInterval(), 120);
        QCOMPARE(first.draftFeatures(), edited);
        QVERIFY(first.branches().isEmpty());
        QVERIFY(!first.branchesLoaded());
        first.saveRepository(first.draftRepository(), first.draftBranch(), first.draftInterval(),
                             first.draftFeatures());
        QTRY_VERIFY_WITH_TIMEOUT(!first.draftOpen(), 18000);

        first.editRepository(1);
        second.reload();
        QTRY_VERIFY(!second.busy());
        second.editRepository(1);
        second.removeRepository();
        QTRY_COMPARE_WITH_TIMEOUT(second.repositoryList()->names(), QStringList{"fixture/alpha"},
                                  18000);
        first.reload();
        QTRY_VERIFY(first.draftNeedsReview());
        QVERIFY(!first.draftCanReview());
        first.reviewDraft(first.draftRepository(), {}, 60, AllFeatures);
        QVERIFY(first.draftNeedsReview());
        first.closeDraft();
        QVERIFY(!first.draftOpen());

        first.disconnectAccount();
        QTRY_COMPARE_WITH_TIMEOUT(first.credential(), Connector::Disconnected, 18000);
        QCOMPARE(first.repositoryList()->names().size(), 1);
        QVERIFY(first.candidateList()->names().isEmpty());
        first.authorize(Replacement);
        QTRY_COMPARE_WITH_TIMEOUT(first.credential(), Connector::Connected, 18000);
        first.editRepository(0);
        first.removeRepository();
        QTRY_VERIFY_WITH_TIMEOUT(first.repositoryList()->names().isEmpty(), 18000);
        QVERIFY(!first.draftOpen());
    }
};

QTEST_GUILESS_MAIN(GitHubTests)
#include "GitHubTests.moc"
