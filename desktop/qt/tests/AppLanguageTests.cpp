#include "AppLanguage.h"

#include <QCoreApplication>
#include <QLocale>
#include <QSettings>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QtTest>

using ctxfab::AppLanguage;

class AppLanguageTests final : public QObject {
    Q_OBJECT

  private slots:
    void selectsAndPersistsLanguages() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        const auto file = directory.filePath(QStringLiteral("ui.ini"));
        {
            QSettings settings(file, QSettings::IniFormat);
            AppLanguage language(settings, QLocale(QStringLiteral("fr_CH")).uiLanguages());
            const QSignalSpy changes(&language, &AppLanguage::changed);
            QCOMPARE(language.selection(), 0);
            QCOMPARE(language.effectiveLanguage(), QStringLiteral("fr"));
            QCOMPARE(language.options().size(), 9);
            QCOMPARE(QCoreApplication::translate("Main", "Settings"), QStringLiteral("Réglages"));
            QCOMPARE(QCoreApplication::translate("TraceAnswerDetails", "%n candidate", nullptr, 1),
                     QStringLiteral("1 candidat"));
            QCOMPARE(QCoreApplication::translate("TraceAnswerDetails", "%n candidates", nullptr, 2),
                     QStringLiteral("2 candidats"));

            language.setSelection(7);
            QCOMPARE(language.effectiveLanguage(), QStringLiteral("ja"));
            QCOMPARE(QCoreApplication::translate("TraceAnswerDetails", "%n candidates", nullptr, 2),
                     QStringLiteral("候補 2 件"));
            QVERIFY(QCoreApplication::translate("Main", "Settings") != QStringLiteral("Settings"));
            QCOMPARE(changes.count(), 1);

            language.setSelection(-1);
            language.setSelection(9);
            language.setSelection(7);
            QCOMPARE(changes.count(), 1);
            QCOMPARE(language.selection(), 7);
        }
        QSettings settings(file, QSettings::IniFormat);
        AppLanguage restored(settings, {QStringLiteral("en")});
        QCOMPARE(restored.selection(), 7);
        QCOMPARE(restored.effectiveLanguage(), QStringLiteral("ja"));
        restored.setSelection(0);
        QCOMPARE(restored.effectiveLanguage(), QStringLiteral("en"));
        QCOMPARE(QCoreApplication::translate("Main", "Settings"), QStringLiteral("Settings"));
    }

    void followsSupportedSystemLocales() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        QSettings settings(directory.filePath(QStringLiteral("ui.ini")), QSettings::IniFormat);
        AppLanguage language(settings, {QStringLiteral("zh-Hant-TW"), QStringLiteral("de-CH")});
        QCOMPARE(language.selection(), 0);
        QCOMPARE(language.effectiveLanguage(), QStringLiteral("de"));

        AppLanguage simplified(settings, {QStringLiteral("zh-Hans-CN"), QStringLiteral("en")});
        QCOMPARE(simplified.effectiveLanguage(), QStringLiteral("zh-Hans"));
    }

    void shipsEveryCatalog() {
        const QTemporaryDir directory;
        QVERIFY(directory.isValid());
        QSettings settings(directory.filePath(QStringLiteral("ui.ini")), QSettings::IniFormat);
        settings.setValue(QStringLiteral("app-language"), QStringLiteral("unsupported"));
        AppLanguage language(settings, {QStringLiteral("nl-NL")});
        QCOMPARE(language.selection(), 0);
        QCOMPARE(language.effectiveLanguage(), QStringLiteral("en"));
        const QStringList codes{QStringLiteral("en"), QStringLiteral("fr"),
                                QStringLiteral("de"), QStringLiteral("es"),
                                QStringLiteral("it"), QStringLiteral("pt"),
                                QStringLiteral("ja"), QStringLiteral("zh-Hans")};
        for (qsizetype index = 0; index < codes.size(); ++index) {
            language.setSelection(static_cast<int>(index + 1));
            QCOMPARE(language.effectiveLanguage(), codes[index]);
            const auto ready = QCoreApplication::translate("ctxfab::Overview", "Ready");
            QCOMPARE(ready == QStringLiteral("Ready"), index == 0);
        }
    }
};

QTEST_GUILESS_MAIN(AppLanguageTests)
#include "AppLanguageTests.moc"
