#include "BackendSupervisor.h"
#include "HistoryPreferences.h"
#include "Preferences.h"

#include <QTemporaryDir>
#include <QtTest>

namespace {

const auto LiveBackend = "CTXFAB_TEST_BACKEND_EXECUTABLE";

} // namespace

class PreferencesTests : public QObject {
    Q_OBJECT

  private slots:
    void keepsSettingsInactiveWhileTheSessionIsUnavailable() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend(
            {.executable = {}, .directory = {}, .externalEndpoint = directory.filePath("missing")});
        ctxfab::Preferences preferences(backend);
        ctxfab::HistoryPreferences history(backend);
        preferences.open();
        history.open();

        QVERIFY(!preferences.ready() && !preferences.busy());
        QVERIFY(!history.ready() && !history.busy());
        QCOMPARE(preferences.feedback(), QStringLiteral("Backend session unavailable"));
        QCOMPARE(history.feedback(), preferences.feedback());
        QVERIFY(!preferences.hasDiagnosticLimit());
        QVERIFY(!preferences.hasArtifactLimit());
        QCOMPARE(preferences.languageChoices(),
                 (QStringList{"Automatic", "English", QStringLiteral("Français")}));

        preferences.setRetentionHours("48");
        preferences.chooseLanguage(1);
        preferences.save();
        history.setRetentionDays(12);
        history.clear();
        QVERIFY(preferences.retentionHours().isEmpty());
        QVERIFY(preferences.languageName().isEmpty());
        QCOMPARE(history.retentionDays(), 30);
    }

    void preservesDraftAndRebasesAfterAConcurrentSave() {
        const auto executable = qEnvironmentVariable(LiveBackend);
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to test live settings");
        }
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend({.executable = executable,
                                           .directory = directory.filePath("profile"),
                                           .externalEndpoint = {}});
        ctxfab::Preferences first(backend);
        ctxfab::Preferences second(backend);
        first.open();
        second.open();
        backend.start();
        QTRY_VERIFY(first.ready() && second.ready());
        QCOMPARE(first.retentionHours(), QStringLiteral("24"));
        QCOMPARE(first.diagnosticLimit(), QStringLiteral("10"));
        QCOMPARE(first.languageName(), QStringLiteral("Automatic"));
        QVERIFY(first.hasArtifactLimit());
        const auto originalArtifactLimit = first.artifactLimit();

        first.setRetentionHours("0");
        first.save();
        QVERIFY(!first.busy());
        QCOMPARE(first.feedback(),
                 QStringLiteral("Diagnostic retention must be between 1 and 168 hours."));
        QCOMPARE(first.retentionHours(), QStringLiteral("0"));

        first.setRetentionHours("48");
        first.setDiagnosticLimit("1001");
        first.save();
        QCOMPARE(first.feedback(),
                 QStringLiteral("Diagnostic storage limit must be between 2 and 1000 MB."));
        first.setDiagnosticLimit("10");
        first.setArtifactLimit("0");
        first.save();
        QCOMPARE(first.feedback(),
                 QStringLiteral("Inference artifact limit must be between 1 and 1000 GB."));
        first.setArtifactLimit("19");
        first.setLanguage("invalid");
        first.save();
        QCOMPARE(first.feedback(), QStringLiteral("Choose a supported answer language."));

        first.setLanguageFilter("ja");
        QCOMPARE(first.languageChoices(), QStringList{QStringLiteral("日本語")});
        first.chooseLanguage(0);
        QCOMPARE(first.language(), QStringLiteral("ja"));
        QCOMPARE(first.languageName(), QStringLiteral("日本語"));
        first.setLanguageFilter(QStringLiteral("日本語"));
        QCOMPARE(first.languageChoices(), QStringList{QStringLiteral("日本語")});
        second.setDiagnosticLimit("1000");
        second.save();
        QTRY_COMPARE(second.feedback(), QStringLiteral("Settings saved."));

        first.save();
        QTRY_VERIFY(first.conflict());
        QCOMPARE(first.retentionHours(), QStringLiteral("48"));
        QCOMPARE(first.language(), QStringLiteral("ja"));
        QCOMPARE(second.artifactLimit(), originalArtifactLimit);

        first.reload();
        QTRY_VERIFY(!first.busy() && !first.conflict());
        QCOMPARE(first.retentionHours(), QStringLiteral("48"));
        QCOMPARE(first.diagnosticLimit(), QStringLiteral("1000"));
        QCOMPARE(first.artifactLimit(), QStringLiteral("19"));
        first.save();
        QTRY_COMPARE(first.feedback(), QStringLiteral("Settings saved."));

        backend.restart();
        QTRY_VERIFY(first.ready() && !first.busy());
        QCOMPARE(first.retentionHours(), QStringLiteral("48"));
        QCOMPARE(first.diagnosticLimit(), QStringLiteral("1000"));
        QCOMPARE(first.artifactLimit(), QStringLiteral("19"));
        QCOMPARE(first.language(), QStringLiteral("ja"));
    }

    void savesHistoryPolicyAcrossRestartAndClearsThroughTheContract() {
        const auto executable = qEnvironmentVariable(LiveBackend);
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to test live history settings");
        }
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        ctxfab::BackendSupervisor backend({.executable = executable,
                                           .directory = directory.filePath("profile"),
                                           .externalEndpoint = {}});
        ctxfab::HistoryPreferences history(backend);
        ctxfab::HistoryPreferences competitor(backend);
        history.open();
        competitor.open();
        backend.start();
        QTRY_VERIFY(history.ready() && competitor.ready());
        QVERIFY(!history.enabled());
        QCOMPARE(history.retentionDays(), 30);

        history.setRetentionDays(0);
        QVERIFY(!history.busy());
        QCOMPARE(history.retentionDays(), 30);
        history.setCustomRetention("12");
        QTRY_COMPARE(history.retentionDays(), 12);

        competitor.setEnabled(true);
        QTRY_VERIFY(competitor.conflict());
        QVERIFY(!competitor.enabled());
        QCOMPARE(competitor.feedback(),
                 QStringLiteral("History settings changed. Reload before editing."));
        competitor.reload();
        QTRY_VERIFY(!competitor.busy() && !competitor.conflict());
        QCOMPARE(competitor.retentionDays(), 12);
        QVERIFY(!competitor.enabled());
        history.setEnabled(true);
        QTRY_VERIFY(history.enabled());
        backend.restart();
        QTRY_VERIFY(history.ready());
        QVERIFY(history.enabled());
        QCOMPARE(history.retentionDays(), 12);

        history.clear();
        QTRY_COMPARE(history.feedback(), QStringLiteral("Answer history cleared."));
        QVERIFY(history.enabled());
        history.setEnabled(false);
        QTRY_VERIFY(!history.enabled() && !history.busy());
        QCOMPARE(history.retentionDays(), 12);
    }
};

QTEST_GUILESS_MAIN(PreferencesTests)
#include "PreferencesTests.moc"
