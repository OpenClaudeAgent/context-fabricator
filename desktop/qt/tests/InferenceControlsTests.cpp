#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "EmbeddingIndex.h"
#include "EmbeddingProfiles.h"
#include "InferenceSetup.h"
#include "LibraryModel.h"
#include "LocalRuntime.h"
#include "LoopbackApiFixture.h"

#include <QProcess>
#include <QTemporaryDir>
#include <QUuid>
#include <QtTest>

namespace {

const QByteArray RuntimeRoute = "/v1/runtime";
const QByteArray IndexRoute = "/v1/retrieval/index";
const QByteArray ModelsRoute = "/v1/retrieval/models";

QJsonObject runtimeStatus(const QString &state) {
    return {{"state", state}, {"archiveBytes", 89'000'000}, {"canDownload", true}};
}

QJsonObject readyRuntime() {
    return {{"state", "ready"}, {"engine", QJsonObject{{"name", "llama.cpp"}, {"build", "12359"}}}};
}

QJsonObject indexSnapshot(bool enabled, qint64 epoch) {
    return {{"profile", "embedding-fixture"}, {"enabled", enabled}, {"epoch", epoch}};
}

QJsonObject profiles(const QString &active = {}) {
    QJsonObject result{{"profiles", QJsonArray{QJsonObject{{"profile", "q8"}, {"label", "Q8_0"}},
                                               QJsonObject{{"profile", "f16"}, {"label", "F16"}}}}};
    if (!active.isEmpty()) {
        result.insert("activeProfile", active);
    }
    return result;
}

bool seedLexicalCorpus(const QString &database) {
    return QProcess::execute("python3", {"-c", QStringLiteral(R"(
import sqlite3
import sys
with sqlite3.connect(sys.argv[1]) as db:
    db.executescript("""
        INSERT INTO collections(name) VALUES ('qt-index-fixture');
        INSERT INTO documents(collection, source, revision, kind, title, content)
        VALUES ('qt-index-fixture', 'src/search.rs', 'fixture-r1', 'history',
                'Index fixture', 'fixturelexicalevidence survives index maintenance.');
    """)
)"),
                                         database}) == 0;
}

QList<ctxfab::openapi::SearchHit> lexicalSearch(const ctxfab::Session &session) {
    ctxfab::LocalApiRequest request;
    auto *api = request.begin(session);
    QSignalSpy hits(api, &ctxfab::openapi::DefaultApi::searchCorpusSignal);
    api->searchCorpus("fixturelexicalevidence", ctxfab::openapi::OptionalParam(QString("bm25")));
    if (hits.empty() && !hits.wait(5000)) {
        return {};
    }
    return hits.constFirst().constFirst().value<QList<ctxfab::openapi::SearchHit>>();
}

} // namespace

class InferenceControlsTests : public QObject {
    Q_OBJECT

  private slots:
    void bindsSetupConfirmationToReviewedSelectionAndFollowsOperation() {
        const QByteArray setup = "/v1/inference/setup";
        const auto selection = QJsonObject{{"answerProfile", "q5"},
                                           {"embedding", true},
                                           {"embeddingProfile", "q8"},
                                           {"runtimeSource", "hosted"}};
        const QString fingerprint(64, 'e');
        const QJsonObject plan{{"selection", selection},
                               {"fingerprint", fingerprint},
                               {"peakAdditionalBytes", 9'007'199'254'740'993LL},
                               {"maximumTransferBytes", 5'000'000'000LL},
                               {"availableBytes", 9'007'199'254'741'000LL},
                               {"canStart", true},
                               {"runtimeRequired", true},
                               {"answerInstalled", false}};
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        fixture.replies[setup] = {{.body = {}, .status = "404 Not Found"}};
        fixture.replies["POST " + setup + "/plan"] = {{.body = plan}};
        fixture.replies["POST " + setup] = {{.body = {{"code", "inference_setup_conflict"},
                                                      {"message", "Review the updated plan."}},
                                             .status = "409 Conflict"}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::InferenceSetup model(backend);
        model.setObserved(true);
        QTRY_VERIFY(model.statusKnown());
        QVERIFY(!model.hasStatus());
        model.review("q5", true, "q8", false);
        QVERIFY(model.reviewing());
        QTRY_VERIFY(model.hasPlan());
        QCOMPARE(model.planAnswerProfile(), QString("q5"));
        QCOMPARE(model.planEmbeddingProfile(), QString("q8"));
        QCOMPARE(model.planPeak(), ctxfab::formatArtifactSize(9'007'199'254'740'993LL));
        QVERIFY(model.canConfirm({}));
        model.confirm({});
        QVERIFY(model.writing());
        QTRY_COMPARE(model.feedback(), QString("Review the updated plan."));
        QVERIFY(!model.hasPlan());
        const auto sent = fixture.requestBodies.value("POST " + setup);
        QCOMPARE(sent.value("selection").toObject(), selection);
        QCOMPARE(sent.value("fingerprint").toString(), fingerprint);
        const auto id = sent.value("operationId").toString();
        QVERIFY(!QUuid(id).isNull());
        QVERIFY(!sent.contains("archivePath"));

        const QJsonObject active{{"operationId", id},
                                 {"phase", "answerWeights"},
                                 {"answerReady", false},
                                 {"selection", selection}};
        fixture.replies[setup] = {{.body = active}};
        QTRY_VERIFY(model.inProgress());
        QCOMPARE(model.activeProfile(), QString("q5"));
        model.setObserved(false);
        auto done = active;
        done.insert("phase", "completed");
        done.insert("answerReady", true);
        fixture.replies[setup] = {{.body = done}};
        QSignalSpy ready(&model, &ctxfab::InferenceSetup::modelsChanged);
        QTRY_VERIFY(model.answerReady());
        QCOMPARE(ready.count(), 1);
        QVERIFY(!model.inProgress());
    }

    void recoversAnArchiveAndRejectsStaleCancellation() {
        const QByteArray setup = "/v1/inference/setup";
        const QString id("1da8a2c6-50cc-4708-83d2-58b9e9691c84");
        const QJsonObject waiting{
            {"operationId", id}, {"phase", "waitingForArchive"}, {"answerReady", false}};
        auto resumed = waiting;
        resumed.insert("phase", "runtime");
        auto cancelled = waiting;
        cancelled.insert("phase", "interrupted");
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        fixture.replies[setup] = {{.body = waiting}};
        fixture.replies["POST " + setup + "/archive"] = {
            {.body = resumed, .status = "202 Accepted"}};
        const auto cancel = setup + "/archive/" + id.toLatin1();
        fixture.replies["DELETE " + cancel] = {{.body = cancelled}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::InferenceSetup model(backend);
        model.setObserved(true);
        QTRY_VERIFY(model.waitingForArchive());
        QCOMPARE(model.operationId(), id);
        model.provideArchive(id, QUrl("https://not-local.invalid/runtime.zip"));
        QCOMPARE(model.feedback(), QString("Choose a local runtime archive."));
        model.cancelArchive("a different operation");
        QVERIFY(!model.writing());
        QVERIFY(!fixture.requests.contains(cancel));
        model.provideArchive(id, QUrl::fromLocalFile("/tmp/runtime pinned.zip"));
        QTRY_VERIFY(!model.waitingForArchive());
        QVERIFY(model.inProgress());
        const QJsonObject archive{{"operationId", id}, {"archivePath", "/tmp/runtime pinned.zip"}};
        QCOMPARE(fixture.requestBodies.value("POST " + setup + "/archive"), archive);
        QTRY_VERIFY(model.waitingForArchive());
        fixture.replies[setup] = {{.body = cancelled}};
        model.cancelArchive(id);
        QTRY_VERIFY(!model.inProgress());
        QVERIFY(fixture.requests.contains(cancel));
        QCOMPARE(model.statusText(), QString("Setup interrupted."));
    }

    void blocksQuotaAndRequiresAnArchiveForLocalSetup() {
        const QByteArray setup = "/v1/inference/setup";
        const auto estimate = "POST " + setup + "/plan";
        const auto confirmation = "POST " + setup;
        const QJsonObject resolved{{"answerProfile", "q4"},
                                   {"embedding", true},
                                   {"embeddingProfile", "q8"},
                                   {"runtimeSource", "localArchive"}};
        const QJsonObject plan{{"selection", resolved},
                               {"fingerprint", QString(64, 'c')},
                               {"peakAdditionalBytes", 580000000},
                               {"maximumTransferBytes", 0},
                               {"availableBytes", 900000000},
                               {"canStart", true},
                               {"runtimeRequired", true},
                               {"answerInstalled", true},
                               {"embeddingInstalled", true}};
        auto blocked = plan;
        blocked.insert("canStart", false);
        blocked.insert("blocker", "storage_limit");
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        fixture.replies[setup] = {{.body = {}, .status = "404 Not Found"}};
        fixture.replies[estimate] = {{.body = blocked}, {.body = plan}};
        fixture.replies[confirmation] = {{.body = {}, .status = "503 Service Unavailable"}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::InferenceSetup model(backend);
        model.setObserved(true);
        QTRY_VERIFY(model.statusKnown());
        model.review("q4", true, {}, true);
        QTRY_VERIFY(model.hasPlan());
        QCOMPARE(model.planEmbeddingProfile(), QString("q8"));
        QCOMPARE(model.planBlocker(),
                 QString("There is not enough available storage for this setup."));
        QVERIFY(!model.planAllowed());
        const auto archive = QUrl::fromLocalFile("/tmp/pinned archive.zip");
        model.confirm(archive);
        QVERIFY(!fixture.requestBodies.contains(confirmation));
        model.discardPlan();
        model.review("q4", true, {}, true);
        QTRY_VERIFY(model.planAllowed());
        QVERIFY(model.planNeedsArchive());
        QVERIFY(!model.planHasTransfer());
        QVERIFY(model.planAnswerInstalled());
        QVERIFY(model.planEmbeddingInstalled());
        model.confirm({});
        QVERIFY(!fixture.requestBodies.contains(confirmation));
        model.confirm(archive);
        QTRY_VERIFY(fixture.requestBodies.contains(confirmation));
        const auto body = fixture.requestBodies.value(confirmation);
        QCOMPARE(body.value("archivePath").toString(), archive.toLocalFile());
        QCOMPARE(body.value("selection").toObject(), resolved);
        QTRY_COMPARE(model.feedback(), QString("Setup failed."));
    }

    void observesBundledRuntimeAndFollowsDownload() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        fixture.replies[RuntimeRoute] = {{.body = runtimeStatus("availableForInstall")}};
        fixture.replies["POST /v1/runtime/download"] = {{.body = runtimeStatus("installing")}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::LocalRuntime runtime(backend);
        QVERIFY(!runtime.hasRuntime());
        runtime.setObserved(true);
        QTRY_VERIFY(runtime.canDownload());
        QVERIFY(runtime.canImport());
        QVERIFY(!runtime.archiveSummary().isEmpty());
        QVERIFY(runtime.needsRestart());
        runtime.install(QUrl("https://example.invalid/runtime.zip"));
        QVERIFY(runtime.feedback().contains("Choose"));
        QVERIFY(!fixture.requests.contains("/v1/runtime/install"));
        runtime.download();
        QTRY_VERIFY(fixture.requests.contains("/v1/runtime/download"));
        QVERIFY(runtime.busy());
        QVERIFY(!runtime.canDownload());
        fixture.replies[RuntimeRoute] = {{.body = readyRuntime()}};
        QTRY_COMPARE(runtime.engine(), QString("llama.cpp"));
        QCOMPARE(runtime.status(), QString("Build 12359"));
        QVERIFY(runtime.hasRuntime());
        QVERIFY(!runtime.busy());
        QVERIFY(!runtime.canImport());
        QVERIFY(!runtime.needsRestart());
        runtime.setObserved(false);
    }

    void importsLocalArchiveAndKeepsProviderFailureVisible() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        auto failed = runtimeStatus("failed");
        failed.insert("error", "Archive failed verification.");
        fixture.replies[RuntimeRoute] = {{.body = runtimeStatus("availableForInstall")}};
        fixture.replies["POST /v1/runtime/install"] = {{.body = failed}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::LocalRuntime runtime(backend);
        runtime.setObserved(true);
        QTRY_VERIFY(runtime.canImport());
        fixture.replies[RuntimeRoute] = {{.body = failed}};
        runtime.install(QUrl::fromLocalFile("/tmp/pinned release.zip"));
        QTRY_VERIFY(fixture.requests.contains("/v1/runtime/install"));
        QTRY_COMPARE(runtime.feedback(), QString("Archive failed verification."));
        QCOMPARE(fixture.requestBodies.value("POST /v1/runtime/install"),
                 (QJsonObject{{"archivePath", "/tmp/pinned release.zip"}}));
        QVERIFY(runtime.canImport());
        fixture.replies["POST /v1/runtime/download"] = {
            {.body = {{"code", "runtime_setup_unavailable"}, {"message", "Package unavailable."}},
             .status = "503 Service Unavailable"}};
        runtime.download();
        QTRY_COMPARE(runtime.feedback(), QString("Package unavailable."));
        QVERIFY(runtime.canDownload());
        fixture.replies[RuntimeRoute] = {{.body = {{"state", "unavailable"}}}};
        QTRY_VERIFY(!runtime.hasRuntime());
        QVERIFY(!runtime.canDownload());
        QVERIFY(runtime.needsRestart());
    }

    void appliesOnlyConfirmedIndexChangesAndKeepsDeletionReceipt() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        constexpr qint64 Epoch = 9'007'199'254'740'992LL;
        fixture.replies[IndexRoute] = {{.body = indexSnapshot(true, Epoch)}};
        fixture.replies["PUT " + IndexRoute] = {
            {.body = {{"code", "embedding_busy"}, {"message", "Index is busy."}},
             .status = "503 Service Unavailable"},
            {.body = indexSnapshot(false, Epoch + 1)}};
        auto deleted = indexSnapshot(false, Epoch + 2);
        deleted.insert("removedVectors", 2);
        deleted.insert("removedBytes", 8192);
        fixture.replies["DELETE " + IndexRoute] = {{.body = deleted}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::EmbeddingIndex index(backend);
        index.setObserved(true);
        QTRY_VERIFY(index.loaded());
        QVERIFY(index.enabled());
        QSignalSpy committed(&index, &ctxfab::EmbeddingIndex::updated);
        index.setEnabled(false);
        QVERIFY(index.enabled());
        QTRY_COMPARE(index.feedback(), QString("Index is busy."));
        QVERIFY(index.enabled());
        QCOMPARE(committed.count(), 0);
        index.setEnabled(false);
        QTRY_VERIFY(!index.enabled());
        QCOMPARE(fixture.requestBodies.value("PUT " + IndexRoute),
                 (QJsonObject{{"enabled", false}}));
        QCOMPARE(index.epoch(), Epoch + 1);
        QCOMPARE(committed.count(), 1);
        index.refresh();
        QTRY_VERIFY(!index.loading());
        QCOMPARE(index.epoch(), Epoch + 1);
        QVERIFY(!index.enabled());
        fixture.replies[IndexRoute] = {{.body = deleted}};
        index.remove();
        QTRY_VERIFY(index.removalSummary().contains("Removed 2 vectors"));
        QCOMPARE(index.epoch(), Epoch + 2);
        QCOMPARE(committed.count(), 2);
        QVERIFY(!index.enabled());
        index.setObserved(false);
        const auto count = fixture.requests.count();
        index.refresh();
        QCOMPARE(fixture.requests.count(), count);
    }

    void usesTheServerProfileOrderAndRecoversFromSelectionConflict() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        fixture.replies[ModelsRoute] = {{.body = profiles()}};
        fixture.replies["PUT " + ModelsRoute] = {
            {.body = {{"code", "embedding_selection_conflict"}, {"message", "Selection is busy."}},
             .status = "409 Conflict"},
            {.body = profiles("f16")}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::EmbeddingProfiles selection(backend);
        selection.setObserved(true);
        QTRY_VERIFY(selection.loaded());
        QCOMPARE(selection.labels(), (QStringList{"Q8_0", "F16"}));
        QCOMPARE(selection.selectedIndex(), -1);
        selection.select(1);
        QTRY_COMPARE(selection.feedback(), QString("Selection is busy."));
        QCOMPARE(selection.selectedIndex(), -1);
        QSignalSpy committed(&selection, &ctxfab::EmbeddingProfiles::selected);
        fixture.replies[ModelsRoute] = {{.body = profiles("f16")}};
        const auto requestsBeforeSelection = fixture.requests.count();
        selection.select(1);
        selection.refresh();
        QTRY_COMPARE(selection.selectedIndex(), 1);
        QTRY_COMPARE(fixture.requests.count(), requestsBeforeSelection + 2);
        QTRY_VERIFY(!selection.busy());
        QCOMPARE(fixture.requestBodies.value("PUT " + ModelsRoute),
                 (QJsonObject{{"profile", "f16"}}));
        QCOMPARE(committed.count(), 1);
        const auto count = fixture.requests.count();
        selection.select(1);
        selection.select(-1);
        QCOMPARE(fixture.requests.count(), count);
    }

    void recoversReadsAndRejectsAnInconsistentDeletionReceipt() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        fixture.replies[RuntimeRoute] = {{.body = {{"state", "unknown"}}}};
        fixture.replies[IndexRoute] = {{.body = {}, .status = "503 Service Unavailable"}};
        fixture.replies[ModelsRoute] = {{.body = profiles("unknown-profile")}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::LocalRuntime runtime(backend);
        ctxfab::EmbeddingIndex index(backend);
        ctxfab::EmbeddingProfiles selection(backend);
        runtime.setObserved(true);
        index.setObserved(true);
        selection.setObserved(true);
        QTRY_VERIFY(!runtime.feedback().isEmpty());
        QVERIFY(!runtime.hasRuntime());
        QTRY_VERIFY(!index.feedback().isEmpty());
        QVERIFY(!index.loaded());
        QTRY_VERIFY(!selection.feedback().isEmpty());
        QVERIFY(!selection.loaded());

        fixture.replies[RuntimeRoute] = {{.body = {{"state", "ready"}}}};
        fixture.replies[IndexRoute] = {{.body = indexSnapshot(true, 10)}};
        fixture.replies[ModelsRoute] = {{.body = profiles("q8")}};
        index.refresh();
        selection.refresh();
        QTRY_COMPARE(runtime.status(), QString("Ready"));
        QCOMPARE(runtime.engine(), QString("Inference engine"));
        QVERIFY(runtime.archiveSummary().isEmpty());
        QTRY_VERIFY(index.loaded());
        QTRY_COMPARE(selection.selectedIndex(), 0);
        QVERIFY(index.feedback().isEmpty());
        auto badReceipt = indexSnapshot(true, 11);
        badReceipt.insert("removedVectors", 1);
        badReceipt.insert("removedBytes", 4096);
        fixture.replies["DELETE " + IndexRoute] = {{.body = badReceipt}};
        index.remove();
        QTRY_VERIFY(index.feedback().contains("Could not update"));
        QVERIFY(index.removalSummary().isEmpty());
        QVERIFY(index.enabled());
        QCOMPARE(index.epoch(), 10);
        selection.setObserved(false);
    }

    void persistsIndexControlsAcrossRustProcessRestart() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the Rust index controls");
        }
        const QTemporaryDir profile;
        QVERIFY(profile.isValid());
        ctxfab::BackendSupervisor backend(
            {.executable = executable, .directory = profile.path(), .externalEndpoint = {}});
        ctxfab::EmbeddingIndex index(backend);
        ctxfab::EmbeddingProfiles selection(backend);
        ctxfab::LocalRuntime runtime(backend);
        index.setObserved(true);
        selection.setObserved(true);
        runtime.setObserved(true);
        backend.start();
        QTRY_VERIFY_WITH_TIMEOUT(index.loaded(), 8000);
        QTRY_VERIFY(selection.loaded());
        QVERIFY(selection.labels().isEmpty());
        QVERIFY(index.enabled());
        QVERIFY(seedLexicalCorpus(profile.filePath("context.sqlite3")));
        index.setEnabled(false);
        QTRY_VERIFY(!index.enabled() && !index.loading());
        const auto disabled = index.epoch();
        index.remove();
        QTRY_VERIFY(index.epoch() > disabled);
        const auto deleted = index.epoch();
        QVERIFY(!index.removalSummary().isEmpty());
        const auto session = backend.session();
        QVERIFY(session);
        QCOMPARE(lexicalSearch(*session).size(), 1);
        backend.restart();
        QVERIFY(!index.loaded());
        QVERIFY(!selection.loaded());
        QTRY_VERIFY_WITH_TIMEOUT(index.loaded(), 10000);
        QTRY_VERIFY(selection.loaded());
        QCOMPARE(index.epoch(), deleted);
        QVERIFY(!index.enabled());
        QVERIFY(index.removalSummary().isEmpty());
        const auto restartedSession = backend.session();
        QVERIFY(restartedSession);
        QCOMPARE(lexicalSearch(*restartedSession).size(), 1);
        index.setEnabled(true);
        QTRY_VERIFY(index.enabled());
    }
};

QTEST_GUILESS_MAIN(InferenceControlsTests)
#include "InferenceControlsTests.moc"
