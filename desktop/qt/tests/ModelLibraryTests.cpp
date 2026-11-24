#include "BackendSupervisor.h"
#include "HubCatalog.h"
#include "LoopbackApiFixture.h"
#include "ModelLibrary.h"

#include <QFile>
#include <QNetworkReply>
#include <QProcess>
#include <QTemporaryDir>
#include <QtTest>

namespace {

using Entry = ctxfab::LibraryModel;
constexpr auto Q5 = "qwen3-4b-instruct-2507-q5-k-m-5bde5e9d-v1";
constexpr qint64 FirstChunk = 1'048'576;

QJsonObject sampleModel(const QString &state, const QString &stage = {}, qint64 downloaded = 0) {
    QJsonObject result{{"profile", "catalog-fixture"},
                       {"kind", "generation"},
                       {"state", state},
                       {"downloadedBytes", downloaded},
                       {"totalBytes", 9'007'199'254'740'993LL}};
    if (!stage.isEmpty()) {
        result.insert("installationStage", stage);
    }
    return result;
}

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
    void start(const QString &executable) {
        QVERIFY(directory_.isValid());
        origin_.start("python3", {"-B", CTXFAB_HUB_FIXTURE, directory_.path(), "--setup-q5"});
        QVERIFY(origin_.waitForStarted());
        QFile file(directory_.filePath("port"));
        QTRY_VERIFY(file.exists());
        QVERIFY(file.open(QIODevice::ReadOnly));
        port_ = QString::fromLatin1(file.readAll()).trimmed();
        QVERIFY(port_.toInt() > 0);
        auto environment = QProcessEnvironment::systemEnvironment();
        environment.insert("CTXFAB_TEST_HUGGING_FACE_PORT", port_);
        server_.setProcessEnvironment(environment);
        server_.start(executable, {directory_.filePath("profile")});
        QVERIFY(server_.waitForStarted());
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(endpoint()), 8000);
    }

    void releaseFirstChunk() const {
        QNetworkAccessManager network;
        const QUrl url(QStringLiteral("http://127.0.0.1:%1/__ui/release-q5").arg(port_));
        auto *reply = network.post(QNetworkRequest(url), QByteArray{});
        QTRY_VERIFY(reply->isFinished());
        QCOMPARE(reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt(), 204);
    }

    [[nodiscard]] QString endpoint() const { return directory_.filePath("profile/endpoint.json"); }
    [[nodiscard]] QString weightFile(const QString &profile) const {
        return directory_.filePath(QStringLiteral("profile/inference/%1.gguf").arg(profile));
    }

  private:
    QTemporaryDir directory_;
    Process origin_;
    Process server_;
    QString port_;
};

} // namespace

class ModelLibraryTests : public QObject {
    Q_OBJECT

  private slots:
    void rendersDownloadPhases_data() {
        QTest::addColumn<QString>("state");
        QTest::addColumn<QString>("stage");
        QTest::addColumn<QString>("expected");
        QTest::addColumn<int>("action");
        QTest::addColumn<bool>("determinate");
        QTest::newRow("present") << QString("installed") << QString{} << QString("Installed")
                                 << int(Entry::Remove) << true;
        QTest::newRow("preparing") << QString("installing") << QString("preparing")
                                   << QString("Preparing download") << int(Entry::Pause) << false;
        QTest::newRow("verifying") << QString("installing") << QString("verifying")
                                   << QString("Verifying model file") << int(Entry::Pause) << false;
        QTest::newRow("transferring") << QString("installing") << QString("transferring")
                                      << QString("Downloading") << int(Entry::Pause) << true;
        QTest::newRow("pausing-transfer") << QString("pausing") << QString("transferring")
                                          << QString("Pausing ·") << int(Entry::NoAction) << true;
        QTest::newRow("pausing-verify")
            << QString("pausing") << QString("verifying") << QString("Pausing download")
            << int(Entry::NoAction) << false;
        QTest::newRow("paused") << QString("paused") << QString{} << QString("Paused")
                                << int(Entry::Resume) << true;
        QTest::newRow("interrupted") << QString("interrupted") << QString{}
                                     << QString("Interrupted") << int(Entry::Resume) << true;
        QTest::newRow("retry") << QString("failed") << QString{} << QString("Download failed")
                               << int(Entry::Download) << true;
        QTest::newRow("absent") << QString("notInstalled") << QString{} << QString("Not installed")
                                << int(Entry::Download) << true;
    }

    void rendersDownloadPhases() {
        QFETCH(QString, state);
        QFETCH(QString, stage);
        QFETCH(QString, expected);
        QFETCH(int, action);
        QFETCH(bool, determinate);
        ctxfab::openapi::ModelDownload download;
        download.fromJsonObject(sampleModel(state, stage, FirstChunk));
        const Entry row(download, {});
        QVERIFY(Entry::accepts(download));
        QCOMPARE(row.label(), QString("catalog-fixture"));
        QVERIFY(row.status().startsWith(expected));
        QCOMPARE(row.action(), action);
        QCOMPARE(row.determinate(), determinate);
        QCOMPARE(row.totalBytes(), 9'007'199'254'740'993LL);
        QVERIFY(row.progress() > 0);
        QVERIFY(row.progress() < 1);
        QVERIFY(row.sourceUrl().isEmpty());
        QCOMPARE(row.canAct(), row.action() != Entry::NoAction);
        QCOMPARE(row.installed(), row.action() == Entry::Remove);
        QCOMPARE(row.actionLabel().isEmpty(), !row.canAct());
    }

    void rendersActionableErrorsAndProtectsAnActiveModel() {
        const QHash<QString, QString> failures{
            {"model_installation_unavailable", "Model installation is currently unavailable."},
            {"model_transfer_failed", "Retry to resume the download."},
            {"model_integrity_failed", "The download did not pass verification."},
            {"model_storage_limit",
             "Free disk space or raise the inference artifact limit in Settings."},
            {QString{}, "Try downloading the model again."}};
        for (auto it = failures.cbegin(); it != failures.cend(); ++it) {
            auto json = sampleModel("failed");
            if (!it.key().isEmpty()) {
                json.insert("error", it.key());
            }
            ctxfab::openapi::ModelDownload download;
            download.fromJsonObject(json);
            const Entry row(download, {});
            QCOMPARE(row.feedback(), it.value());
            QCOMPARE(row.actionLabel(), QString("Retry"));
        }
        auto installed = sampleModel("installed");
        installed.insert("deletionBlock", "Selected for local answers");
        ctxfab::openapi::ModelDownload download;
        download.fromJsonObject(installed);
        const Entry row(download, {});
        QVERIFY(!row.canAct());
        QCOMPARE(row.actionLabel(), QString("Delete Model…"));
        QCOMPARE(row.feedback(), QString("Selected for local answers"));
    }

    void reloadsCatalogAfterManagedBackendRestart() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise model library recovery");
        }
        const QTemporaryDir profile;
        QVERIFY(profile.isValid());
        ctxfab::BackendSupervisor backend(
            {.executable = executable, .directory = profile.path(), .externalEndpoint = {}});
        ctxfab::ModelLibrary library(backend);
        library.setQuery("q5_k_m");
        library.open();
        QVERIFY(!library.ready());
        QVERIFY(library.modelFolder().isEmpty());
        QVERIFY(!library.catalogReady());
        backend.start();
        QTRY_VERIFY_WITH_TIMEOUT(library.catalogReady(), 8000);
        QCOMPARE(library.answerModels()->entryAt(0).profile(), QString(Q5));
        const auto oldSession = backend.session();
        QVERIFY(oldSession);

        backend.restart();
        QVERIFY(!library.ready());
        QVERIFY(library.empty());
        QVERIFY(!library.catalogReady());
        QVERIFY(library.removalMessage().isEmpty());
        QTRY_VERIFY_WITH_TIMEOUT(library.catalogReady(), 10000);
        QCOMPARE(library.answerModels()->entryAt(0).profile(), QString(Q5));
        QVERIFY(backend.session()->token != oldSession->token);
        QVERIFY(library.catalogFeedback().isEmpty());
        library.close();
    }

    void preservesCatalogAcrossReadFailuresAndReplacesStorageEstimate() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        const QByteArray catalog = "/v1/model-downloads";
        const QByteArray storage = "/v1/inference/storage";
        const auto available = sampleModel("notInstalled");
        const QJsonObject capacity{{"usedBytes", 1000},       {"limitBytes", 4000},
                                   {"reservedBytes", 500},    {"availableBytes", 2500},
                                   {"volumeFreeBytes", 8000}, {"systemReserveBytes", 500}};
        const QJsonObject peak{
            {"profile", "catalog-fixture"}, {"peakBytes", 3500}, {"fits", false}};
        fixture.replies[catalog] = {{.body = {}, .array = QJsonArray{available}}};
        const QJsonObject runtime{
            {"localImport", QJsonObject{{"peakBytes", 2000}, {"fits", true}}},
            {"hostedDownload", QJsonObject{{"peakBytes", 3500}, {"fits", false}}}};
        fixture.replies[storage] = {
            {.body = {{"capacity", capacity}, {"runtime", runtime}, {"models", QJsonArray{peak}}}}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::ModelLibrary library(backend);
        QVERIFY(library.empty());
        library.open();
        QTRY_VERIFY(library.catalogReady());
        QTRY_VERIFY(!library.storageSummary().isEmpty());
        QVERIFY(!library.empty());
        QVERIFY(library.storageDetails().contains("Reserved"));
        QVERIFY(library.runtimeImportEstimate().contains("ZIP import peak"));
        QVERIFY(library.runtimeDownloadEstimate().contains("Insufficient available space"));
        auto *answers = library.answerModels();
        const auto row = answers->entryAt(0);
        QVERIFY(row.peakSpace().contains("Insufficient available space"));
        QCOMPARE(answers->data(answers->index(0), answers->roleNames().key("entry")).value<Entry>(),
                 row);
        QVERIFY(!answers->data({}, Qt::DisplayRole).isValid());
        QSignalSpy modelChanges(answers, &QAbstractItemModel::modelReset);
        library.retranslate();
        QCOMPARE(modelChanges.count(), 1);

        fixture.replies[catalog] = {{.body = {}, .array = QJsonArray{available, available}}};
        fixture.replies[storage] = {{.body = {}, .status = "503 Service Unavailable"}};
        library.refresh();
        QTRY_VERIFY(!library.catalogFeedback().isEmpty());
        QTRY_VERIFY(!library.storageFeedback().isEmpty());
        QVERIFY(answers->entryAt(0).peakSpace().isEmpty());
        QCOMPARE(answers->count(), 1);
        QVERIFY(library.storageSummary().isEmpty());
        QVERIFY(library.runtimeImportEstimate().isEmpty());

        fixture.replies[catalog] = {{.body = {}, .array = QJsonArray{available}}};
        fixture.replies["POST " + catalog] = {
            {.body = {{"code", "model_storage_limit"}, {"message", "Storage cap reached."}},
             .status = "503 Service Unavailable"}};
        library.refresh();
        QTRY_VERIFY(library.catalogFeedback().isEmpty());
        library.act(row);
        QTRY_COMPARE(library.feedback(), QString("Storage cap reached."));
        QCOMPARE(library.feedbackProfile(), row.profile());
        QVERIFY(library.pendingProfile().isEmpty());
        library.close();
        const auto observed = fixture.requests.size();
        library.refresh();
        QCOMPARE(fixture.requests.size(), observed);
        QVERIFY(!library.catalogReady());
    }

    void keepsSetupChoicesIndependentOfTheLibraryFilter() {
        const auto setupModel = [](const QString &profile, const QString &kind,
                                   const QString &role) {
            auto row = sampleModel("notInstalled");
            row.insert("profile", profile);
            row.insert("kind", kind);
            row.insert("setupRole", role);
            return row;
        };
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        const QJsonArray models{setupModel("q5", "generation", "alternativeAnswer"),
                                setupModel("q4", "generation", "recommendedAnswer"),
                                setupModel("hub", "generation", ""),
                                setupModel("q8", "embedding", "optionalEmbedding"),
                                setupModel("bad-role", "embedding", "recommendedAnswer")};
        fixture.replies["/v1/model-downloads"] = {{.body = {}, .array = models}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::ModelLibrary library(backend);
        library.open();
        QTRY_VERIFY(library.catalogReady());
        QCOMPARE(library.setupAnswers()->count(), 2);
        QCOMPARE(library.setupAnswers()->entryAt(0).profile(), QString("q4"));
        QVERIFY(library.setupAnswers()->entryAt(0).recommendedForSetup());
        QVERIFY(library.setupEmbeddings()->containsProfile("q8"));
        QCOMPARE(library.setupEmbeddings()->count(), 1);
        library.setQuery("hub");
        QCOMPARE(library.answerModels()->count(), 1);
        QCOMPARE(library.setupAnswers()->count(), 2);
        QVERIFY(library.setupAnswers()->containsProfile("q5"));
        QVERIFY(!library.setupAnswers()->containsProfile("hub"));
    }

    void reviewsPinnedEmbeddingFilesAndHandlesConflict() {
        using Hub = ctxfab::HubCatalog;
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        const QString repository = "fixture/embedding";
        const QString revision(40, 'a');
        const QString digest(64, 'b');
        const QString filename = "embedding-Q8_0.gguf";
        const QByteArray download = "POST /v1/model-catalog/downloads";
        const QJsonObject result{{"repository", repository},
                                 {"revision", revision},
                                 {"url", "https://huggingface.co/fixture/embedding"},
                                 {"kind", "embedding"}};
        QJsonObject variant{
            {"file", "unverified-Q8_0.gguf"}, {"bytes", 1000}, {"quantization", "Q8_0"}};
        auto verified = variant;
        verified.insert("file", filename);
        verified.insert("sha256", digest);
        const auto variantsRoute = "/v1/model-catalog/variants?repository=fixture%2Fembedding"
                                   "&revision=" +
                                   revision.toLatin1() + "&kind=embedding";
        fixture.replies["/v1/model-catalog?query=models&kind=embedding"] = {
            {.body = {}, .array = QJsonArray{result}}};
        fixture.replies[variantsRoute] = {{.body = {}, .array = QJsonArray{variant, verified}}};
        fixture.replies[download] = {
            {.body = {{"code", "model_catalog_conflict"}, {"message", "File changed."}},
             .status = "409 Conflict"},
            {.body = {{"profile", "embedding-fixture"}}, .status = "202 Accepted"}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        Hub hub(backend);
        hub.open();
        hub.setQuery("ignored");
        hub.search();
        QVERIFY(hub.busy());
        hub.selectKind(Hub::Embedding);
        QVERIFY(!hub.busy());
        QVERIFY(!hub.hasResults());
        QCOMPARE(hub.query(), QString("ignored"));
        hub.setQuery("  models  ");
        hub.search();
        QTRY_COMPARE(hub.repositories().size(), 1);
        QCOMPARE(hub.repositories().first().label(), repository);
        hub.openRepository(0);
        QCOMPARE(hub.busyText(), QString("Checking compatible files…"));
        QCOMPARE(hub.repositoryUrl(), QUrl("https://huggingface.co/fixture/embedding"));
        hub.back();
        QCOMPARE(hub.query(), QString("  models  "));
        QCOMPARE(hub.repositories().size(), 1);
        QVERIFY(hub.repositoryName().isEmpty());
        hub.openRepository(0);
        QTRY_COMPARE(hub.variants().size(), 2);
        QVERIFY(!hub.variants().first().selectable());
        hub.selectVariant(0);
        QVERIFY(!hub.canDownload());
        hub.selectVariant(1);
        QVERIFY(hub.canDownload());
        hub.download();
        QCOMPARE(hub.busyText(), QString("Preparing verified download…"));
        QVERIFY(hub.addingDownload());
        hub.close();
        hub.back();
        hub.selectKind(Hub::Generation);
        QVERIFY(hub.active());
        QCOMPARE(hub.kind(), Hub::Embedding);
        QTRY_VERIFY(hub.feedback().contains("File changed."));
        QVERIFY(hub.feedback().startsWith("Search again"));
        QVERIFY(hub.canDownload());
        const QJsonObject expected{{"repository", repository},
                                   {"revision", revision},
                                   {"kind", "embedding"},
                                   {"file", filename},
                                   {"sha256", digest}};
        QCOMPARE(fixture.requestBodies.value(download), expected);
        QSignalSpy accepted(&hub, &Hub::downloadAccepted);
        hub.download();
        QTRY_COMPARE(accepted.size(), 1);
        QCOMPARE(accepted.constFirst().constFirst().toString(), QString("embedding-fixture"));
        QVERIFY(!hub.active());
        QVERIFY(hub.query().isEmpty());
    }

    void distinguishesEmptyHubResultsFromFailedResponses() {
        ctxfab::test::LoopbackApiFixture fixture;
        QVERIFY(fixture.start());
        const QByteArray search = "/v1/model-catalog?query=catalog&kind=generation";
        fixture.replies[search] = {
            {.body = {{"message", "wrong response shape"}}},
            {.body = {}, .array = QJsonArray{}},
            {.body = {{"code", "provider_unavailable"}, {"message", "Provider unavailable."}},
             .status = "503 Service Unavailable"}};
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpointPath()});
        ctxfab::HubCatalog hub(backend);
        hub.open();
        hub.setQuery("   ");
        hub.search();
        QVERIFY(!hub.busy());
        QVERIFY(hub.busyText().isEmpty());
        QCOMPARE(fixture.requests.count(search), 0);
        hub.setQuery("catalog");
        hub.search();
        QCOMPARE(hub.busyText(), QString("Searching Hugging Face…"));
        QTRY_COMPARE(hub.feedback(), QString("Couldn’t Load Models"));
        QVERIFY(!hub.hasResults());
        hub.search();
        QTRY_VERIFY(hub.hasResults());
        QVERIFY(hub.feedback().isEmpty());
        QVERIFY(hub.repositories().isEmpty());
        hub.openRepository(0);
        QVERIFY(hub.repositoryUrl().isEmpty());
        hub.search();
        QTRY_COMPARE(hub.feedback(), QString("Provider unavailable."));
        QVERIFY(!hub.hasResults());
        hub.close();
        QVERIFY(!hub.active());
        QVERIFY(hub.feedback().isEmpty());
    }

    void searchesDownloadsAndPreservesIdentityAcrossRemovalConfirmation() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise model removal");
        }
        Fixture fixture;
        fixture.start(executable);
        QVERIFY(!QTest::currentTestFailed());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        ctxfab::ModelLibrary library(backend);
        QCOMPARE(library.modelFolder().toLocalFile() + "/sample.gguf",
                 fixture.weightFile("sample"));
        library.open();
        QTRY_VERIFY(library.catalogReady());
        library.setQuery("fixture/catalog-qwen");
        QSignalSpy updated(&library, &ctxfab::ModelLibrary::installedModelsChanged);
        ctxfab::HubCatalog hub(backend);
        connect(&hub, &ctxfab::HubCatalog::downloadAccepted, &library,
                &ctxfab::ModelLibrary::refresh);
        hub.open();
        hub.setQuery("catalog");
        hub.search();
        QTRY_COMPARE(hub.repositories().size(), 1);
        QCOMPARE(hub.repositories().first().label(), QString("fixture/catalog-qwen"));
        hub.openRepository(0);
        QTRY_COMPARE(hub.variants().size(), 1);
        QCOMPARE(hub.variants().first().label(), QString("Qwen3-4B-Instruct-2507-Q4_K_M.gguf"));
        hub.selectVariant(0);
        QSignalSpy accepted(&hub, &ctxfab::HubCatalog::downloadAccepted);
        hub.download();
        QTRY_COMPARE_WITH_TIMEOUT(accepted.size(), 1, 10000);
        QVERIFY(!hub.active());
        const auto profile = accepted.constFirst().constFirst().toString();
        QVERIFY(!profile.isEmpty());
        const auto path = fixture.weightFile(profile);
        QTRY_COMPARE_WITH_TIMEOUT(library.answerModels()->entryAt(0).action(), Entry::Remove,
                                  12000);
        QCOMPARE(library.answerModels()->entryAt(0).profile(), profile);
        QVERIFY(QFile::exists(path));
        QCOMPARE(updated.count(), 1);

        library.act(library.answerModels()->entryAt(0));
        QTRY_VERIFY(!library.removalMessage().isEmpty());
        QVERIFY(library.removalMessage().contains(profile));
        library.cancelRemoval();
        QVERIFY(library.removalMessage().isEmpty());
        QVERIFY(QFile::exists(path));

        library.act(library.answerModels()->entryAt(0));
        QTRY_VERIFY(!library.removalMessage().isEmpty());
        const auto previous = path + QStringLiteral(".original");
        QVERIFY(QFile::rename(path, previous));
        QVERIFY(QFile::copy(previous, path));
        library.confirmRemoval();
        QTRY_VERIFY(library.removalMessage().contains("Review the updated removal plan"));
        QVERIFY(QFile::exists(path));
        library.confirmRemoval();
        QTRY_COMPARE_WITH_TIMEOUT(library.answerModels()->entryAt(0).action(), Entry::Download,
                                  8000);
        QVERIFY(!QFile::exists(path));
        QVERIFY(QFile::exists(previous));
        QCOMPARE(updated.count(), 2);
    }

    void filtersAvailableModelsAndControlsTheLiveDownload() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the Rust model library");
        }
        Fixture fixture;
        fixture.start(executable);
        QVERIFY(!QTest::currentTestFailed());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = fixture.endpoint()});
        ctxfab::ModelLibrary library(backend);

        library.open();
        QTRY_VERIFY(library.catalogReady());
        QVERIFY(library.answerModels()->count() >= 2);
        QVERIFY(library.embeddingModels()->count() > 0);
        QVERIFY(library.downloads()->count() == 0);
        library.setQuery("  q5_k_m  ");
        QCOMPARE(library.answerModels()->count(), 1);
        QCOMPARE(library.answerModels()->entryAt(0).profile(), QString(Q5));
        QCOMPARE(library.answerModels()->entryAt(0).totalBytes(), 2'889'514'080LL);
        QVERIFY(!library.answerModels()->entryAt(0).sourceUrl().isEmpty());
        QVERIFY(library.embeddingModels()->count() == 0);

        library.act(library.answerModels()->entryAt(0));
        QTRY_COMPARE_WITH_TIMEOUT(library.downloads()->count(), 1, 8000);
        QCOMPARE(library.downloads()->entryAt(0).action(), Entry::Pause);
        QCOMPARE(library.downloads()->entryAt(0).downloadedBytes(), 0);
        library.setQuery("unmatched");
        QVERIFY(library.answerModels()->count() == 0);
        QCOMPARE(library.downloads()->entryAt(0).profile(), QString(Q5));
        QSignalSpy reset(library.downloads(), &QAbstractItemModel::modelReset);
        QSignalSpy changed(library.downloads(), &QAbstractItemModel::dataChanged);
        fixture.releaseFirstChunk();
        QVERIFY(!QTest::currentTestFailed());
        QTRY_COMPARE_WITH_TIMEOUT(library.downloads()->entryAt(0).downloadedBytes(), FirstChunk,
                                  10000);
        QVERIFY(!changed.isEmpty());
        QVERIFY(reset.isEmpty());

        library.act(library.downloads()->entryAt(0));
        QTRY_COMPARE_WITH_TIMEOUT(library.downloads()->entryAt(0).action(), Entry::Resume, 8000);
        QVERIFY(library.downloads()->entryAt(0).canDiscard());
        library.act(library.downloads()->entryAt(0));
        QTRY_COMPARE_WITH_TIMEOUT(library.downloads()->entryAt(0).action(), Entry::Pause, 8000);
        QTRY_VERIFY_WITH_TIMEOUT(library.downloads()->entryAt(0).downloadedBytes() > FirstChunk,
                                 8000);
        library.act(library.downloads()->entryAt(0));
        QTRY_COMPARE_WITH_TIMEOUT(library.downloads()->entryAt(0).action(), Entry::Resume, 8000);
        library.discard(Q5);
        QTRY_VERIFY(library.downloads()->count() == 0);
        library.setQuery("q5_k_m");
        QCOMPARE(library.answerModels()->entryAt(0).downloadedBytes(), 0);
        QCOMPARE(library.answerModels()->entryAt(0).action(), Entry::Download);
        QVERIFY(library.feedback().isEmpty());
        library.close();
    }
};

QTEST_GUILESS_MAIN(ModelLibraryTests)
#include "ModelLibraryTests.moc"
