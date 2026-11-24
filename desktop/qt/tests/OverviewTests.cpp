#include "BackendSupervisor.h"
#include "Overview.h"

#include <QDir>
#include <QFile>
#include <QJsonDocument>
#include <QJsonObject>
#include <QProcess>
#include <QTcpServer>
#include <QTcpSocket>
#include <QTemporaryDir>
#include <QtTest>

#include <csignal>

namespace {

constexpr auto ReadyJson =
    R"({"status":"ready","apiVersion":1,"connectedSources":2,)"
    R"("indexedDocuments":7,"collectedDocuments":9,"outboundAttempts":11,)"
    R"("outboundPending":3,"outboundUncertain":4,"inference":"notConfigured",)"
    R"("retrieval":{"state":"incomplete","dimensions":384,"indexed":9007199254740993,"total":9007199254740994}})";

constexpr auto FixtureToken = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
constexpr auto LegacyJson =
    R"({"status":"ready","apiVersion":1,"connectedSources":2,)"
    R"("indexedDocuments":7,"outboundAttempts":11,"inference":"notConfigured"})";

bool writeEndpoint(const QString &path, quint16 port) {
    QFile file(path);
    const QJsonObject endpoint{
        {"apiVersion", 1},
        {"processId", 123},
        {"url", QStringLiteral("http://127.0.0.1:%1").arg(port)},
        {"token", QString::fromLatin1(FixtureToken)},
    };
    const auto json = QJsonDocument(endpoint).toJson();
    return file.open(QIODevice::WriteOnly) &&
           file.setPermissions(QFileDevice::ReadOwner | QFileDevice::WriteOwner) &&
           file.write(json) == json.size();
}

} // namespace

class OverviewTests : public QObject {
    Q_OBJECT

  private slots:
    void usesTheGeneratedClient_data() {
        QTest::addColumn<QByteArray>("status");
        QTest::addColumn<QByteArray>("body");
        QTest::addColumn<QString>("expected");
        QTest::addColumn<QString>("embedding");

        const auto statusWithEmbedding = [](const char *state, const char *display) {
            QTest::newRow(state) << QByteArray("200 OK")
                                 << QByteArray(ReadyJson).replace("incomplete", state)
                                 << QString("Ready") << QString::fromLatin1(display);
        };
        statusWithEmbedding("incomplete", "Indexing");
        statusWithEmbedding("ready", "Ready");
        statusWithEmbedding("disabled", "Off");
        statusWithEmbedding("notConfigured", "Not configured");
        statusWithEmbedding("unrecognized", "Unknown");
        QTest::newRow("optional-fields-absent")
            << QByteArray("200 OK") << QByteArray(LegacyJson) << QString("Ready") << QString{};
        QTest::newRow("malformed") << QByteArray("200 OK") << QByteArray("{}")
                                   << QString("Invalid response from the backend") << QString{};
        QTest::newRow("unauthorized") << QByteArray("401 Unauthorized") << QByteArray("{}")
                                      << QString("Backend request failed") << QString{};
        QTest::newRow("redirect") << QByteArray("302 Found") << QByteArray(ReadyJson)
                                  << QString("Invalid response from the backend") << QString{};
    }

    void usesTheGeneratedClient() {
        QFETCH(QByteArray, status);
        QFETCH(QByteArray, body);
        QFETCH(QString, expected);
        QFETCH(QString, embedding);

        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        QTcpServer server;
        QTcpServer redirectTarget;
        QVERIFY(server.listen(QHostAddress::LocalHost));
        QVERIFY(redirectTarget.listen(QHostAddress::LocalHost));
        const auto endpoint = directory.filePath("endpoint.json");
        QVERIFY(writeEndpoint(endpoint, server.serverPort()));

        QByteArray request;
        connect(&server, &QTcpServer::newConnection, this, [&] {
            auto *connection = server.nextPendingConnection();
            connect(connection, &QTcpSocket::readyRead, this, [&, connection] {
                request += connection->readAll();
                if (!request.contains("\r\n\r\n")) {
                    return;
                }
                connection->write("HTTP/1.1 " + status +
                                  "\r\nContent-Type: application/json"
                                  "\r\nContent-Length: " +
                                  QByteArray::number(body.size()) +
                                  "\r\nConnection: close\r\nLocation: http://127.0.0.1:" +
                                  QByteArray::number(redirectTarget.serverPort()) +
                                  "/other\r\n\r\n" + body);
                connection->disconnectFromHost();
            });
        });

        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = endpoint});
        ctxfab::Overview overview(backend);
        QVERIFY(!overview.managed());
        QVERIFY(!overview.canRestart());
        overview.refresh();
        overview.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!overview.busy(), 5000);
        QVERIFY(request.startsWith("GET /v1/status HTTP/1.1\r\n"));
        QVERIFY(request.toLower().contains(QByteArray("\r\nauthorization: bearer ") + FixtureToken +
                                           "\r\n"));
        QCOMPARE(overview.serviceState(), expected);
        QCOMPARE(overview.embeddingState(), embedding);
        QCOMPARE(overview.ready(), expected == "Ready");
        QVERIFY(!redirectTarget.hasPendingConnections());
        if (overview.ready()) {
            QCOMPARE(overview.indexedDocuments(), 7);
            QCOMPARE(overview.connectedSources(), 2);
            QCOMPARE(overview.outboundAttempts(), 11);
        }
        if (body == LegacyJson) {
            QVERIFY(overview.collectedDocuments().isEmpty());
            QVERIFY(overview.pendingOutcomes().isEmpty());
            QVERIFY(overview.uncertainOutcomes().isEmpty());
        }
        if (!embedding.isEmpty()) {
            QCOMPARE(overview.collectedDocuments(), QStringLiteral("9"));
            QCOMPARE(overview.pendingOutcomes(), QStringLiteral("3"));
            QCOMPARE(overview.uncertainOutcomes(), QStringLiteral("4"));
            const QLocale locale;
            const auto progress =
                QStringLiteral("%1 / %2 passages")
                    .arg(locale.toString(9007199254740993LL), locale.toString(9007199254740994LL));
            QCOMPARE(overview.embeddingProgress(),
                     embedding == "Not configured" ? QString{} : progress);
        }
    }

    void reportsAnUnavailableExternalSession() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = directory.filePath("missing")});
        ctxfab::Overview overview(backend);
        overview.refresh();
        QCOMPARE(overview.serviceState(), QStringLiteral("Backend session unavailable"));
        QVERIFY(!overview.ready());
        QVERIFY(!overview.busy());
        QVERIFY(overview.embeddingProgress().isEmpty());
    }

    void supervisesAndRecoversTheLiveRustService() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to run against the Rust service");
        }
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        const auto profile = directory.filePath("backend");
        const auto endpoint = QDir(profile).filePath("endpoint.json");
        const ctxfab::BackendSupervisor::Options options{
            .executable = executable, .directory = profile, .externalEndpoint = {}};
        {
            ctxfab::BackendSupervisor backend(options);
            ctxfab::Overview overview(backend);
            QVERIFY(overview.managed());
            backend.start();
            QTRY_VERIFY_WITH_TIMEOUT(overview.ready(), 8000);
            QCOMPARE(overview.indexedDocuments(), 0);
            const auto original = ctxfab::Session::read(endpoint);
            QVERIFY(original.has_value());

            ctxfab::BackendSupervisor competitor(options);
            competitor.start();
            QTRY_COMPARE(competitor.state(), ctxfab::BackendSupervisor::State::Failed);
            QCOMPARE(
                competitor.error(),
                QStringLiteral("Another Context Fabricator backend is using this data directory."));
            QVERIFY(!competitor.session());
            QVERIFY(overview.ready());

            QCOMPARE(::kill(static_cast<pid_t>(original->processId), SIGKILL), 0);
            QTRY_VERIFY_WITH_TIMEOUT(!overview.ready() && overview.canRestart(), 5000);
            QCOMPARE(backend.state(), ctxfab::BackendSupervisor::State::Failed);
            QVERIFY(QFile::exists(endpoint));
            overview.restart();
            QTRY_VERIFY_WITH_TIMEOUT(overview.ready(), 8000);
            const auto restarted = ctxfab::Session::read(endpoint);
            QVERIFY(restarted.has_value());
            QVERIFY(original->processId != restarted->processId);
            QVERIFY(original->token != restarted->token);

            overview.restart();
            QVERIFY(!overview.ready());
            QTRY_VERIFY_WITH_TIMEOUT(overview.ready(), 8000);
            const auto gracefulRestart = ctxfab::Session::read(endpoint);
            QVERIFY(gracefulRestart.has_value());
            QVERIFY(restarted->token != gracefulRestart->token);
        }
        QVERIFY(!QFile::exists(endpoint));
    }

    void surfacesALaunchFailureAndAllowsRetry() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend({.executable = directory.filePath("missing-helper"),
                                           .directory = directory.filePath("backend"),
                                           .externalEndpoint = {}});
        ctxfab::Overview overview(backend);
        backend.start();
        QTRY_VERIFY(overview.canRestart());
        QVERIFY(!overview.ready());
        QCOMPARE(backend.state(), ctxfab::BackendSupervisor::State::Failed);
        QVERIFY(!overview.serviceState().isEmpty());
    }
};

QTEST_GUILESS_MAIN(OverviewTests)
#include "OverviewTests.moc"
