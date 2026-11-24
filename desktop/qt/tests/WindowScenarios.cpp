#include <QtCore/QCoreApplication>
#include <QtCore/QElapsedTimer>
#include <QtCore/QFile>
#include <QtCore/QSettings>
#include <QtCore/QTemporaryFile>
#include <QtCore/QTimer>
#include <QtCore/QUrl>
#include <QtCore/QtMath>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtGui/QStyleHints>
#include <QtQml/QQmlListReference>
#include <QtQuick/QQuickItem>
#include <QtQuick/QQuickWindow>
#include <QtTest/QSignalSpy>
#include <QtTest/QTest>

namespace {

template <typename Probe> auto await(Probe probe, const char *description, int limit = 18000) {
    QElapsedTimer elapsed;
    elapsed.start();
    do {
        if (auto value = probe())
            return value;
        QTest::qWait(20);
    } while (elapsed.elapsed() < limit);
    qFatal("Expected %s", description);
}

QQuickItem *find(QQuickItem *root, const QString &name) {
    if (!root->isVisible())
        return nullptr;
    if (root->objectName() == name)
        return root;
    for (auto *child : root->childItems())
        if (auto *match = find(child, name))
            return match;
    return nullptr;
}

QQuickItem *item(QQuickWindow &window, const char *name) {
    return await([&] { return find(window.contentItem(), QString::fromUtf8(name)); }, name);
}

void click(QQuickWindow &window, const char *name) {
    auto *control = item(window, name);
    QPointF point;
    await(
        [&] {
            point = control->mapToScene(QPointF(control->width() / 2, control->height() / 2));
            return control->isEnabled() && control->width() > 0 && control->height() > 0 &&
                   QRectF(0, 0, window.width(), window.height()).contains(point);
        },
        name);
    QTest::mouseClick(&window, Qt::LeftButton, Qt::NoModifier, point.toPoint());
}

void capture(QQuickWindow &window, const char *filename) {
    QTest::qWait(130);
    const auto path =
        qEnvironmentVariable("CTXFAB_WINDOW_EVIDENCE") + '/' + QString::fromUtf8(filename);
    const auto frame = window.grabWindow();
    if (!frame.save(path))
        qFatal("Could not capture %s", filename);

    // Sample the drawn surfaces. Small bright areas are expected for text and icons.
    const auto sample = frame.scaled(160, 160, Qt::KeepAspectRatio);
    int bright = 0;
    for (int y = 0; y < sample.height(); ++y) {
        for (int x = 0; x < sample.width(); ++x) {
            bright += qGray(sample.pixel(x, y)) >= 160;
        }
    }
    const int pixels = sample.width() * sample.height();
    if (bright * 10 > pixels || bright == 0)
        qFatal("%s must show visible content on dark surfaces (%d/%d bright pixels)", filename,
               bright, pixels);
}

void type(QQuickWindow &window, const QByteArray &text) {
    for (char key : text)
        QTest::keyClick(&window, key);
}

void ask(QQuickWindow &window, const QByteArray &question) {
    click(window, "answersTab");
    click(window, "answerQuestion");
    auto *editor = item(window, "answerQuestion");
    if (!QMetaObject::invokeMethod(editor, "selectAll"))
        qFatal("Could not select the previous question");
    type(window, question);
    click(window, "askLocally");
}

void assertTraceOpened(QQuickWindow &window) {
    auto *requests = window.property("answerRequests").value<QObject *>();
    auto *inspector = window.property("traceInspector").value<QObject *>();
    const auto trace = requests->property("latestTraceId").toString();
    if (trace.size() != 32)
        qFatal("The latest request has no trace");
    click(window, "viewLatestAnswerTrace");
    await([&] { return inspector->property("traceId").toString() == trace; }, "exact latest trace");
    item(window, "traceInspectorId");
}

void verifyDiagnosticRequests(QQuickWindow &window) {
    click(window, "askQuestionFromOverview");
    item(window, "answerQuestion");
    ask(window, "What is the retry delay?");
    const auto *answer = item(window, "answerResultText");
    await([answer] { return answer->property("text").toString().contains("47 seconds"); },
          "answer from the fixture corpus");
    capture(window, "18b-answers-wide.png");
    click(window, "diagnosticsTab");
    if (item(window, "diagnosticLatestAnswerQuestion")->property("text").toString() !=
        "What is the retry delay?")
        qFatal("Latest request lost its question");
    capture(window, "01-latest.png");
    assertTraceOpened(window);
    capture(window, "02-exact-trace.png");
    click(window, "traceInspectorClose");

    ask(window, "Hold retry cooldown");
    click(window, "diagnosticsTab");
    item(window, "diagnosticAnswerInProgress");
    if (find(window.contentItem(), "latestAnswerRequest"))
        qFatal("A stale request is shown while the new question is pending");
    capture(window, "03-pending.png");
    click(window, "answersTab");
    click(window, "cancelAnswer");
    click(window, "diagnosticsTab");
    await(
        [&] {
            auto *question = find(window.contentItem(), "diagnosticLatestAnswerQuestion");
            return question && question->property("text").toString() == "Hold retry cooldown";
        },
        "cancelled latest request");

    ask(window, "orbitalzebramarker");
    item(window, "answerResultText");
    click(window, "diagnosticsTab");
    auto *requests = window.property("answerRequests").value<QObject *>();
    await([&] { return requests->property("latestAbstained").toBool(); },
          "abstained latest request");
    capture(window, "04-abstained-and-history.png");
}

void verifyCompactDiagnostics(QQuickWindow &window) {
    auto *language = window.property("appLanguage").value<QObject *>();
    if (!language->setProperty("selection", 2))
        qFatal("Could not apply French to Diagnostics");
    window.resize(460, 650);
    capture(window, "05-compact-fr-dark.png");
    auto *tabs = item(window, "diagnosticTabs");
    const auto *requestsTab = item(window, "diagnosticRequestsTab");
    auto *usageTab = item(window, "diagnosticUsageTab");
    const auto usageCorner =
        usageTab->mapToItem(tabs, QPointF(usageTab->width(), usageTab->height()));
    if (usageCorner.x() > tabs->width() + 1 || usageCorner.y() > tabs->height() ||
        usageTab->mapToScene(QPointF()).y() <= requestsTab->mapToScene(QPointF()).y())
        qFatal("The translated tabs do not fit inside two rows.");
    click(window, "diagnosticUsageTab");
    await([tabs] { return tabs->property("currentIndex").toInt() == 5; },
          "second-row Answer Usage tab");
    usageTab->forceActiveFocus();
    QTest::keyClick(&window, Qt::Key_Right);
    await([tabs] { return tabs->property("currentIndex").toInt() == 0; },
          "keyboard tab navigation");
    if (!requestsTab->property("checked").toBool())
        qFatal("Requests is missing its selected state after keyboard navigation");
    item(window, "diagnosticLatestAnswerQuestion");
    assertTraceOpened(window);
    capture(window, "06-compact-fr-trace.png");
    click(window, "traceInspectorClose");
    window.resize(400, 650);
    const QQmlListReference sections(tabs, "sections");
    if (!sections.isValid() || sections.count() != 6)
        qFatal("The tab bar lost a diagnostic section");
    const auto languages = language->property("options").toStringList();
    if (languages.size() != 9)
        qFatal("Expected system selection and eight interface languages");
    for (int locale = 1; locale < languages.size(); ++locale) {
        if (!language->setProperty("selection", locale) ||
            language->property("selection").toInt() != locale)
            qFatal("Could not switch to interface locale %d", locale);
        QTest::qWait(120);
        QList<QRectF> occupied;
        const QRectF bounds(-0.5, -0.5, tabs->width() + 1, tabs->height() + 1);
        for (qsizetype index = 0; index < sections.count(); ++index) {
            const auto *section = qobject_cast<QQuickItem *>(sections.at(index));
            const auto paintedArea =
                section->mapRectToItem(tabs, QRectF(0, 0, section->width(), section->height()));
            if (!bounds.contains(paintedArea))
                qFatal("A tab clips at 400 px for locale %d: %s", locale,
                       qUtf8Printable(section->property("text").toString()));
            for (const auto &other : occupied)
                if (other.intersects(paintedArea))
                    qFatal("Translated tabs overlap for locale %d", locale);
            occupied.append(paintedArea);
        }
        qInfo() << "All diagnostic tabs fit at 400 px" << languages.at(locale);
    }
}

void verifySettingsLayout(QQuickWindow &window) {
    auto *language = window.property("appLanguage").value<QObject *>();
    if (!language->setProperty("selection", 1))
        qFatal("Could not apply English to the settings window");
    window.resize(1280, 820);
    click(window, "settingsTab");
    const auto *answerLanguage = item(window, "answerLanguage");
    await([answerLanguage] { return answerLanguage->isEnabled(); }, "loaded settings");
    const auto *page = item(window, "settingsPage");
    const auto *column = item(window, "settingsColumn");
    capture(window, "07-settings-wide.png");
    click(window, "answerLanguage");
    const auto *search = item(window, "answerLanguageSearch");
    await([search] { return search->hasActiveFocus(); }, "focused language filter");
    type(window, "japan");
    const auto *preferences = window.property("preferences").value<QObject *>();
    await(
        [preferences] {
            return preferences->property("languageChoices").toStringList().size() == 1;
        },
        "one matching answer language");
    capture(window, "08-settings-language.png");
    QTest::keyClick(&window, Qt::Key_Escape);
    await([&] { return !find(window.contentItem(), "answerLanguageSearch"); }, "dismissed picker");
    const auto center = column->mapToItem(page, QPointF(column->width() / 2, 0));
    if (qAbs(column->width() - 760) > 1 || qAbs(center.x() - page->width() / 2) > 1)
        qFatal("Expected a centered 760 px settings column, found %.1f px wide", column->width());

    if (!language->setProperty("selection", 2))
        qFatal("Could not apply French to the settings window");
    window.resize(460, 650);
    capture(window, "09-settings-compact-fr-dark.png");
    if (qAbs(column->width() - page->property("availableWidth").toReal()) > 1)
        qFatal("The compact settings column does not fill the available space");
    if (find(window.contentItem(), "appearance"))
        qFatal("Settings still shows the obsolete appearance selector");

    click(window, "settingsManageModels");
    item(window, "modelLibraryFilter");
    capture(window, "09b-model-library.png");
    QTest::keyClick(&window, Qt::Key_Escape);
    await([&] { return !find(window.contentItem(), "modelLibraryFilter"); }, "closed library");
    if (!language->setProperty("selection", 1))
        qFatal("Could not apply English to the model library");
    window.resize(1180, 820);
    click(window, "settingsManageModels");
    item(window, "modelLibraryFilter");
    capture(window, "09c-model-library-wide.png");
    QTest::keyClick(&window, Qt::Key_Escape);
    await([&] { return !find(window.contentItem(), "modelLibraryFilter"); }, "closed wide library");
}

void openGitHub(QQuickWindow &window) {
    auto *language = window.property("appLanguage").value<QObject *>();
    if (!language->setProperty("selection", 1))
        qFatal("Could not set the connector interface to English");
    window.resize(1280, 820);
    click(window, "connectorsTab");
    auto *provider = item(window, "githubManage");
    if (find(window.contentItem(), QStringLiteral("githubToken")))
        qFatal("The account token field must stay inside the GitHub account dialog");
    const auto *directory = item(window, "connectorDirectoryColumn");
    await([directory] { return qAbs(directory->width() - 760) <= 1; },
          "centered connector directory");
    capture(window, "10-connectors.png");
    provider->forceActiveFocus();
    QTest::keyClick(&window, Qt::Key_Space);
    item(window, "githubConnect");
    const auto *workspace = item(window, "githubWorkspaceColumn");
    await([workspace] { return qAbs(workspace->width() - 760) <= 1; }, "centered GitHub workspace");
    capture(window, "11-github-disconnected.png");
}

void connectGitHubAccount(QQuickWindow &window) {
    click(window, "githubConnect");
    item(window, "githubToken");
    capture(window, "12-github-account.png");
    click(window, "githubToken");
    type(window, "github_pat_fixture_primary");
    click(window, "githubAuthorize");
    auto *github = window.property("github").value<QObject *>();
    await([github] { return github->property("login").toString() == "fixture-user"; },
          "connected GitHub fixture account");
    click(window, "githubAccountDone");
    await([&] { return !find(window.contentItem(), QStringLiteral("githubToken")); },
          "dismissed account dialog");
}

void addGitHubRepositories(QQuickWindow &window) {
    click(window, "githubAddRepository");
    item(window, "githubCandidate/fixture/alpha");
    click(window, "githubMoreCandidates");
    item(window, "githubCandidate/fixture/beta");
    const auto *privateMarker =
        find(window.contentItem(), QStringLiteral("githubCandidateVisibility/fixture/alpha"));
    if (!privateMarker || !privateMarker->property("privateRepository").toBool())
        qFatal("The candidate picker must identify a private repository");
    const auto *publicMarker = item(window, "githubCandidateVisibility/fixture/beta");
    if (publicMarker->property("privateRepository").toBool())
        qFatal("The candidate picker marked a public repository as private");
    click(window, "githubCandidateSearch");
    QTest::keyClick(&window, Qt::Key_Down);
    const auto *alpha = item(window, "githubCandidate/fixture/alpha");
    await([alpha] { return alpha->property("highlighted").toBool(); },
          "keyboard selection in the repository picker");
    capture(window, "13-github-picker.png");
    QTest::keyClick(&window, Qt::Key_Return);
    if (item(window, "githubRepository")->property("text").toString() != "fixture/alpha")
        qFatal("The picker opened the wrong repository");
    click(window, "githubSaveRepository");
    item(window, "githubConfigured/fixture/alpha");

    click(window, "githubAddRepository");
    const auto *more = item(window, "githubMoreCandidates");
    await([more] { return more->isEnabled(); }, "loaded first repository page");
    if (find(window.contentItem(), QStringLiteral("githubCandidate/fixture/alpha")))
        qFatal("A configured repository is still offered in the picker");
    click(window, "githubCandidateUrlMode");
    click(window, "githubCandidateUrl");
    type(window, "fixture/beta");
    capture(window, "14-github-url-entry.png");
    click(window, "githubContinueRepository");
    if (!item(window, "githubActionsFeature")->property("checked").toBool())
        click(window, "githubActionsFeature");
    click(window, "githubSaveRepository");
    item(window, "githubConfigured/fixture/beta");
    if (item(window, "githubConfiguredBranch/fixture/beta")->property("text").toString() != "trunk")
        qFatal("The configured repository row lost its tracked branch");
    if (item(window, "githubConfiguredWarning/fixture/beta")->property("text").toString() !=
        "Actions access unverified")
        qFatal("The configured repository row lost its Actions permission warning");
    await([&] { return !find(window.contentItem(), QStringLiteral("githubRepository")); },
          "closed repository dialog after saving");
}

void verifyConfiguredRepositoryFilter(QQuickWindow &window) {
    window.requestActivate();
    await([&] { return window.isActive(); }, "active GitHub window after closing its dialog");
    auto *back = item(window, "githubBack");
    back->forceActiveFocus();
    await([back] { return back->hasActiveFocus(); }, "focus inside the GitHub workspace");
    QTest::keySequence(&window, QKeySequence::Find);
    const auto *filter = item(window, "githubConfiguredFilter");
    await([filter] { return filter->hasActiveFocus(); },
          "Find shortcut for configured repositories");
    type(window, "BETA");
    if (find(window.contentItem(), QStringLiteral("githubConfigured/fixture/alpha")))
        qFatal("The filter left a nonmatching repository visible");
    capture(window, "15-github-filtered.png");
    click(window, "githubConfigured/fixture/beta");
    const auto *repository = item(window, "githubRepository");
    if (repository->property("text").toString() != "fixture/beta")
        qFatal("The filtered row opened a different repository");
    QTest::keyClick(&window, Qt::Key_Escape);
    await([&] { return !find(window.contentItem(), QStringLiteral("githubRepository")); },
          "dismissed repository settings");

    auto *language = window.property("appLanguage").value<QObject *>();
    if (!language->setProperty("selection", 2))
        qFatal("Could not apply French to the GitHub workspace");
    window.resize(460, 650);
    const auto *warning = item(window, "githubConfiguredWarning/fixture/beta");
    await([warning] { return warning->property("text").toString() != "Actions access unverified"; },
          "translated repository access warning");
    capture(window, "16-github-metadata-compact.png");

    click(window, "githubConfiguredFilter");
    if (!QMetaObject::invokeMethod(item(window, "githubConfiguredFilter"), "selectAll"))
        qFatal("Could not select the previous filter");
    type(window, "unlisted");
    item(window, "githubNoConfiguredMatches");
    capture(window, "17-github-filtered-compact.png");
    click(window, "githubBack");
    await([&] { return item(window, "githubManage")->hasActiveFocus(); },
          "restored focus on the GitHub connector");
}

void verifyAnswerSurfaces(QQuickWindow &window) {
    ask(window, "What is the retry delay?");
    const auto *answer = item(window, "answerResultText");
    await([answer] { return answer->property("text").toString().contains("47 seconds"); },
          "cited answer in the compact window");
    capture(window, "18-answers.png");
    click(window, "answerHistory");
    item(window, "answerHistoryType");
    capture(window, "19-answer-history.png");
    QTest::keyClick(&window, Qt::Key_Escape);
    await([&] { return !find(window.contentItem(), "answerHistoryType"); },
          "closed answer history");
}

QRectF sceneBounds(QQuickItem &item) {
    return item.mapRectToScene(QRectF(0, 0, item.width(), item.height()));
}

void requireSameBounds(const QRectF &before, QQuickItem &control, const char *context) {
    const auto after = sceneBounds(control);
    if (before != after)
        qFatal("%s shifted from (%.2f, %.2f, %.2f, %.2f) to (%.2f, %.2f, %.2f, %.2f)", context,
               before.x(), before.y(), before.width(), before.height(), after.x(), after.y(),
               after.width(), after.height());
}

void verifyPipelineSuite(QQuickWindow &window, QObject &pipeline) {
    QTemporaryFile suite;
    const QByteArray suiteJson = R"({
        "documents": [{"path":"docs/cooldown.md","text":"The retry cooldown is 47 seconds."}],
        "questions": [
            {"title":"Retry delay","query":"What is the retry delay?","expected":"47 seconds."},
            {"title":"Unrelated question","query":"orbitalzebramarker"}
        ]
    })";
    if (!suite.open() || suite.write(suiteJson) != suiteJson.size() || !suite.flush())
        qFatal("Could not write the synthetic benchmark suite");
    const auto url = QUrl::fromLocalFile(suite.fileName());
    if (!QMetaObject::invokeMethod(&pipeline, "importSuite", Qt::DirectConnection,
                                   Q_ARG(QUrl, url)))
        qFatal("Could not import the synthetic benchmark suite");
    click(window, "pipelineBenchmarkTab");
    auto *runner = item(window, "runBenchmarkSuite");
    const auto runBounds = sceneBounds(*runner);
    auto *list = item(window, "pipelineBenchmarkList");
    const auto listBounds = sceneBounds(*list);
    click(window, "runBenchmarkSuite");
    requireSameBounds(runBounds, *runner, "Run All while running");
    requireSameBounds(listBounds, *list, "benchmark list while running");
    await(
        [&] {
            return pipeline.property("finishedCount").toInt() == 2 &&
                   !pipeline.property("busy").toBool();
        },
        "both benchmark outcomes");
    requireSameBounds(runBounds, *runner, "Run All after completion");
    requireSameBounds(listBounds, *list, "benchmark list after completion");
    if (item(window, "benchmarkResultStatus-0")->property("text").toString() != "Published")
        qFatal("The benchmark question with indexed evidence was not published");
    if (item(window, "benchmarkResultStatus-1")->property("text").toString() !=
        "Insufficient evidence")
        qFatal("The benchmark question outside the corpus did not abstain");
    capture(window, "24-pipeline-results.png");
    click(window, "inspectBenchmarkResult");
    item(window, "runPipelineQuestion");
}

void verifyPipeline(QQuickWindow &window) {
    auto *language = window.property("appLanguage").value<QObject *>();
    if (!language->setProperty("selection", 1))
        qFatal("Could not select English for the benchmark");
    window.resize(1080, 780);
    await([&] { return window.contentItem()->size() == QSizeF(1080, 780); },
          "pipeline window content at its configured size");
    click(window, "answersTab");
    click(window, "openPipelineInspector");
    item(window, "pipelineStage-2");
    auto *pipeline = window.property("pipelineInspector").value<QObject *>();
    auto *question = item(window, "pipelineQuestion");
    click(window, "pipelineQuestion");
    if (!QMetaObject::invokeMethod(question, "selectAll"))
        qFatal("Could not select the pipeline query");
    type(window, "What is the retry delay?");
    auto *run = item(window, "runPipelineQuestion");
    // Measure the painted dialog after the compositor has applied the window resize.
    QSignalSpy presented(&window, &QQuickWindow::frameSwapped);
    window.update();
    await([&] { return !presented.isEmpty(); }, "the painted pipeline inspector");
    const auto questionBounds = sceneBounds(*question);
    const auto runBounds = sceneBounds(*run);
    click(window, "runPipelineQuestion");
    requireSameBounds(questionBounds, *question, "question editor while running");
    requireSameBounds(runBounds, *run, "Run while running");
    await(
        [pipeline] {
            return pipeline->property("traceId").toString().size() == 32 &&
                   !pipeline->property("busy").toBool();
        },
        "pipeline trace from the Rust answer");
    requireSameBounds(questionBounds, *question, "question editor after completion");
    requireSameBounds(runBounds, *run, "Run after completion");
    await(
        [&] {
            return item(window, "traceExcerptText")
                ->property("text")
                .toString()
                .contains("47 seconds");
        },
        "retrieved passage from the fixture corpus");
    capture(window, "20-pipeline-retrieval.png");
    click(window, "pipelineStage-5");
    if (find(window.contentItem(), "pipelineExactPrompt"))
        qFatal("The raw prompt should be collapsed by default");
    click(window, "pipelineModelPassage-0");
    await(
        [&] {
            return item(window, "traceExcerptText")
                ->property("text")
                .toString()
                .contains("47 seconds");
        },
        "actual passage sent to the model");
    capture(window, "21-pipeline-model-input.png");
    click(window, "pipelinePromptDisclosure");
    if (!item(window, "pipelineExactPrompt")->property("text").toString().contains("47 seconds"))
        qFatal("Expanded prompt lost the exact passage");
    click(window, "pipelinePromptDisclosure");

    click(window, "pipelineQuestion");
    if (!QMetaObject::invokeMethod(question, "selectAll"))
        qFatal("Could not select the previous question");
    type(window, "Hold retry cooldown");
    click(window, "runPipelineQuestion");
    auto *loader = item(window, "pipelineCenteredLoader");
    const auto loaderCenter = sceneBounds(*loader).center();
    const auto frameCenter = sceneBounds(*item(window, "pipelineStageFrame")).center();
    if (qAbs(loaderCenter.x() - frameCenter.x()) > 1 ||
        qAbs(loaderCenter.y() - frameCenter.y()) > 25)
        qFatal("The loader is not centered in the stage content");
    requireSameBounds(questionBounds, *question, "question editor while loading");
    capture(window, "25-pipeline-loading.png");
    click(window, "stopPipelineRequest");
    await([pipeline] { return !pipeline->property("busy").toBool(); },
          "cancelled pipeline request");

    click(window, "pipelineBenchmarkTab");
    item(window, "benchmarkQuestion-0");
    if (item(window, "runBenchmarkSuite")->isEnabled())
        qFatal("Benchmark can run without activation");
    capture(window, "22-pipeline-benchmarks.png");
    click(window, "pipelineSourcesTab");
    click(window, "pipelineActionsSourceTab");
    item(window, "pipelineDownloadAllActions");
    capture(window, "23-pipeline-actions.png");
    verifyPipelineSuite(window, *pipeline);

    window.resize(440, 670);
    if (!language->setProperty("selection", 2))
        qFatal("Could not select French for the compact pipeline");
    item(window, "pipelineStage");
    click(window, "pipelineSourcesTab");
    auto *download = item(window, "pipelineDownloadAllActions");
    auto *frame = item(window, "pipelineSourcesFrame");
    auto *close = item(window, "closePipelineInspector");
    await(
        [&] {
            const auto frameBounds = sceneBounds(*frame);
            const auto actionBounds = sceneBounds(*download);
            return !actionBounds.isEmpty() && frameBounds.right() <= window.width() - 12 &&
                   actionBounds.right() <= frameBounds.right() &&
                   actionBounds.bottom() <= frameBounds.top() &&
                   sceneBounds(*close).right() <= frameBounds.right();
        },
        "the compact source actions and header to fit above the content");
    capture(window, "26-pipeline-sources-compact-fr.png");
    click(window, "closePipelineInspector");
}

void runWindowScenarios() {
    auto *window = await(
        []() -> QQuickWindow * {
            for (auto *candidate : QGuiApplication::allWindows())
                if (candidate->objectName() == "mainWindow")
                    return qobject_cast<QQuickWindow *>(candidate);
            return nullptr;
        },
        "main window");
    await([&] { return window->isExposed(); }, "exposed main window");
    if (window->color().lightness() >= 128)
        qFatal("The saved light preference changed the application canvas");
#ifdef Q_OS_MACOS
    if (QGuiApplication::platformName() == QStringLiteral("cocoa") &&
        QGuiApplication::styleHints()->colorScheme() != Qt::ColorScheme::Dark)
        qFatal("Cocoa did not adopt the dark color scheme");
#endif
    capture(*window, "00-overview-startup.png");
    await([] { return QFile::exists(qEnvironmentVariable("CTXFAB_WINDOW_READY")); },
          "seeded corpus");
    if (!qEnvironmentVariableIsEmpty("CTXFAB_WINDOW_PIPELINE_ONLY")) {
        verifyPipeline(*window);
        QCoreApplication::exit(0);
        return;
    }
    auto *language = window->property("appLanguage").value<QObject *>();
    if (!language->setProperty("selection", 1))
        qFatal("Could not select English for the wide captures");
    auto *overview = window->property("overview").value<QObject *>();
    await([overview] { return !overview->property("busy").toBool(); },
          "initial Overview request to finish");
    if (!QMetaObject::invokeMethod(overview, "refresh"))
        qFatal("Could not refresh Overview after seeding the fixture");
    await(
        [overview] {
            return overview->property("ready").toBool() &&
                   overview->property("indexedDocuments").toInt() == 1;
        },
        "seeded document in Overview");
    capture(*window, "00b-overview-ready.png");
    verifyDiagnosticRequests(*window);
    verifyCompactDiagnostics(*window);
    verifySettingsLayout(*window);
    openGitHub(*window);
    connectGitHubAccount(*window);
    addGitHubRepositories(*window);
    verifyConfiguredRepositoryFilter(*window);
    verifyAnswerSurfaces(*window);
    verifyPipeline(*window);
    QCoreApplication::exit(0);
}

void install() {
    if (qEnvironmentVariableIsEmpty("CTXFAB_WINDOW_READY") ||
        qEnvironmentVariableIsEmpty("CTXFAB_WINDOW_EVIDENCE") ||
        qEnvironmentVariableIsEmpty("XDG_CONFIG_HOME"))
        qFatal("Run the window test with run_window_scenarios.py");
    QSettings::setDefaultFormat(QSettings::IniFormat);
    QSettings::setPath(QSettings::IniFormat, QSettings::UserScope,
                       qEnvironmentVariable("XDG_CONFIG_HOME"));
    QCoreApplication::setApplicationName(QStringLiteral("Context Fabricator"));
    QCoreApplication::setOrganizationName(QStringLiteral("Context Fabricator"));
    QCoreApplication::setOrganizationDomain(QStringLiteral("qt.contextfabricator.org"));
    QSettings legacy;
    legacy.setValue(QStringLiteral("appearance"), QStringLiteral("light"));
    legacy.sync();
    QTimer::singleShot(0, runWindowScenarios);
}
} // namespace

Q_COREAPP_STARTUP_FUNCTION(install)
