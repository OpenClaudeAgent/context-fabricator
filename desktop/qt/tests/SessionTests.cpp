#include "Session.h"

#include <QFile>
#include <QTemporaryDir>
#include <QtTest>

#include <sys/stat.h>

using ctxfab::Session;

class SessionTests : public QObject {
    Q_OBJECT

  private slots:
    void acceptsOnlyTheVersionedLoopbackEndpoint_data() {
        QTest::addColumn<QByteArray>("url");
        QTest::addColumn<int>("version");
        QTest::addColumn<bool>("accepted");

        QTest::newRow("loopback") << QByteArray("http://127.0.0.1:45123") << 1 << true;
        QTest::newRow("remote") << QByteArray("http://192.0.2.1:45123") << 1 << false;
        QTest::newRow("path") << QByteArray("http://127.0.0.1:45123/v1") << 1 << false;
        QTest::newRow("no-port") << QByteArray("http://127.0.0.1") << 1 << false;
        QTest::newRow("unknown-version") << QByteArray("http://127.0.0.1:45123") << 2 << false;
    }

    void acceptsOnlyTheVersionedLoopbackEndpoint() {
        QFETCH(QByteArray, url);
        QFETCH(int, version);
        QFETCH(bool, accepted);

        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        QFile file(directory.filePath("endpoint.json"));
        QVERIFY(file.open(QIODevice::WriteOnly));
        QVERIFY(file.setPermissions(QFileDevice::ReadOwner | QFileDevice::WriteOwner));
        file.write(QByteArray(R"({"apiVersion":)") + QByteArray::number(version) +
                   R"(,"processId":123,"url":")" + url + R"(","token":")" + QByteArray(64, 'a') +
                   R"("})");
        file.close();

        QCOMPARE(Session::read(file.fileName()).has_value(), accepted);
    }

    void rejectsPublicAndLinkedEndpointFiles() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        QFile file(directory.filePath("endpoint.json"));
        QVERIFY(file.open(QIODevice::WriteOnly));
        const QByteArray document =
            "{\"apiVersion\":1,\"processId\":123,\"url\":\"http://127.0.0.1:45123\",\"token\":\"" +
            QByteArray(64, 'a') + "\"}";
        QCOMPARE(file.write(document), document.size());
        file.close();

        QVERIFY(::chmod(QFile::encodeName(file.fileName()).constData(), 0644) == 0);
        QVERIFY(!Session::read(file.fileName()).has_value());

        QVERIFY(::chmod(QFile::encodeName(file.fileName()).constData(), 0600) == 0);
        const auto link = directory.filePath("linked.json");
        QVERIFY(file.link(link));
        QVERIFY(!Session::read(link).has_value());
        QVERIFY(Session::read(file.fileName()).has_value());

        const auto parent = QFile::encodeName(directory.path());
        QVERIFY(::chmod(parent.constData(), 0755) == 0);
        QVERIFY(!Session::read(file.fileName()).has_value());
        QVERIFY(::chmod(parent.constData(), 0700) == 0);
        QVERIFY(!Session::read(directory.filePath("missing.json")).has_value());
    }
};

QTEST_GUILESS_MAIN(SessionTests)
#include "SessionTests.moc"
