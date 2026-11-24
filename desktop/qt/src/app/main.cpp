#include "AnswerHistory.h"
#include "AnswerUsageModel.h"
#include "Answers.h"
#include "AppLanguage.h"
#include "BackendSupervisor.h"
#include "DiagnosticAnswerRequests.h"
#include "DiagnosticLogs.h"
#include "Diagnostics.h"
#include "EmbeddingIndex.h"
#include "EmbeddingProfiles.h"
#include "GitHubActionsActivity.h"
#include "GitHubConnector.h"
#include "GitHubGraphBrowser.h"
#include "HistoryPreferences.h"
#include "HubCatalog.h"
#include "InferenceSetup.h"
#include "LocalRuntime.h"
#include "ModelLibrary.h"
#include "Overview.h"
#include "PipelineInspector.h"
#include "Preferences.h"
#include "TraceAnswer.h"
#include "TraceInspector.h"

#include <QCommandLineParser>
#include <QDir>
#include <QFileInfo>
#include <QFontDatabase>
#include <QGuiApplication>
#include <QLocale>
#include <QQmlApplicationEngine>
#include <QQuickStyle>
#include <QSettings>
#include <QStyleHints>
#include <QWindow>
#include <QtQml/QQmlExtensionPlugin>

Q_IMPORT_QML_PLUGIN(ContextFabricatorPlugin)

namespace {

void configureDesktop() {
#ifdef Q_OS_LINUX
    // Desktop portals can draw dialogs in the host theme. Use Qt Quick dialogs on Linux.
    QCoreApplication::setAttribute(Qt::AA_DontUseNativeDialogs);
#endif
    QGuiApplication::setDesktopFileName(QStringLiteral("org.contextfabricator.qt"));
    QGuiApplication::styleHints()->setColorScheme(Qt::ColorScheme::Dark);
    QQuickStyle::setStyle(QStringLiteral("Basic"));
}

void configureTheme(QQmlApplicationEngine &engine) {
    if (auto *theme = engine.singletonInstance<QObject *>(QStringLiteral("ContextFabricator"),
                                                          QStringLiteral("Theme"))) {
        theme->setProperty("fixedFont",
                           QFontDatabase::systemFont(QFontDatabase::FixedFont).family());
    }
}

} // namespace

int main(int argc, char *argv[]) {
    const QGuiApplication app(argc, argv);
    QCoreApplication::setApplicationName(QStringLiteral("Context Fabricator"));
    QCoreApplication::setOrganizationName(QStringLiteral("Context Fabricator"));
    QCoreApplication::setOrganizationDomain(QStringLiteral("qt.contextfabricator.org"));

    configureDesktop();

    QCommandLineParser parser;
    parser.addHelpOption();
    const QCommandLineOption endpoint(QStringLiteral("endpoint-file"),
                                      QStringLiteral("Read an existing local backend session."),
                                      QStringLiteral("path"));
    const QCommandLineOption directory(
        QStringLiteral("data-directory"), QStringLiteral("Override the profile directory."),
        QStringLiteral("path"), ctxfab::BackendSupervisor::defaultDataDirectory());
    const QCommandLineOption helper(
        QStringLiteral("backend"), QStringLiteral("Override the installed backend executable."),
        QStringLiteral("path"),
        QDir(QCoreApplication::applicationDirPath()).filePath(QStringLiteral("ctxfab-server")));
    parser.addOptions({endpoint, directory, helper});
    parser.process(app);
    if (parser.isSet(endpoint) && (parser.isSet(directory) || parser.isSet(helper))) {
        qCritical("--endpoint-file cannot be combined with --backend or --data-directory.");
        return 2;
    }

    QSettings settings;
    ctxfab::AppLanguage appLanguage(settings, QLocale::system().uiLanguages());
    ctxfab::BackendSupervisor backend{
        {.executable = QFileInfo(parser.value(helper)).absoluteFilePath(),
         .directory = QDir(parser.value(directory)).absolutePath(),
         .externalEndpoint = parser.value(endpoint)}};
    ctxfab::Overview overview(backend);
    ctxfab::Answers answers(backend);
    ctxfab::PipelineInspector pipelineInspector(backend, settings);
    ctxfab::ModelLibrary modelLibrary(backend);
    ctxfab::HubCatalog hubCatalog(backend);
    ctxfab::InferenceSetup inferenceSetup(backend);
    ctxfab::LocalRuntime localRuntime(backend);
    ctxfab::EmbeddingIndex embeddingIndex(backend);
    ctxfab::EmbeddingProfiles embeddingProfiles(backend);
    QObject::connect(&modelLibrary, &ctxfab::ModelLibrary::installedModelsChanged, &answers,
                     &ctxfab::Answers::refreshModels);
    QObject::connect(&modelLibrary, &ctxfab::ModelLibrary::installedModelsChanged,
                     &embeddingProfiles, &ctxfab::EmbeddingProfiles::refresh);
    QObject::connect(&hubCatalog, &ctxfab::HubCatalog::downloadAccepted, &modelLibrary,
                     &ctxfab::ModelLibrary::refresh);
    QObject::connect(&inferenceSetup, &ctxfab::InferenceSetup::modelsChanged, &answers,
                     &ctxfab::Answers::refreshModels);
    QObject::connect(&inferenceSetup, &ctxfab::InferenceSetup::modelsChanged, &embeddingProfiles,
                     &ctxfab::EmbeddingProfiles::refresh);
    QObject::connect(&inferenceSetup, &ctxfab::InferenceSetup::modelsChanged, &modelLibrary,
                     &ctxfab::ModelLibrary::refresh);
    QObject::connect(&embeddingIndex, &ctxfab::EmbeddingIndex::updated, &overview,
                     &ctxfab::Overview::refresh);
    QObject::connect(&embeddingProfiles, &ctxfab::EmbeddingProfiles::selected, &overview,
                     &ctxfab::Overview::refresh);
    QObject::connect(&embeddingProfiles, &ctxfab::EmbeddingProfiles::selected, &modelLibrary,
                     &ctxfab::ModelLibrary::refresh);
    ctxfab::Preferences preferences(backend);
    ctxfab::HistoryPreferences history(backend);
    ctxfab::AnswerHistory answerHistory(backend, history);
    QObject::connect(&history, &ctxfab::HistoryPreferences::historyCleared, &answers,
                     &ctxfab::Answers::discardSessionQuestions);
    QObject::connect(&answers, &ctxfab::Answers::requestFinished, &answerHistory,
                     &ctxfab::AnswerHistory::refresh);
    ctxfab::GitHubConnector github(backend);
    QObject::connect(&github, &ctxfab::GitHubConnector::repositoryPurged, &answers,
                     &ctxfab::Answers::repositoryPurged);
    QObject::connect(&github, &ctxfab::GitHubConnector::repositoryPurged, &answerHistory,
                     &ctxfab::AnswerHistory::invalidate);
    ctxfab::GitHubActionsActivity githubActions(backend, github);
    ctxfab::GitHubGraphBrowser graph(backend);
    ctxfab::Diagnostics diagnostics(backend);
    ctxfab::DiagnosticLogs diagnosticLogs(backend);
    ctxfab::DiagnosticAnswerRequests answerRequests(backend, answers);
    ctxfab::AnswerUsageModel answerUsage(backend);
    QObject::connect(&diagnostics, &ctxfab::Diagnostics::controlCompleted, &diagnosticLogs,
                     &ctxfab::DiagnosticLogs::refresh);
    QObject::connect(&diagnostics, &ctxfab::Diagnostics::controlCompleted, answerRequests.feed(),
                     &ctxfab::DiagnosticLogs::refresh);
    QObject::connect(&answers, &ctxfab::Answers::requestFinished, &answerUsage,
                     &ctxfab::AnswerUsageModel::refresh);
    QObject::connect(&diagnostics, &ctxfab::Diagnostics::controlCompleted, &answerUsage,
                     &ctxfab::AnswerUsageModel::refresh);
    ctxfab::TraceInspector traceInspector(backend);
    ctxfab::TraceAnswer traceAnswer(backend, answers, history);
    const auto synchronizeTraceAnswer = [&traceInspector, &traceAnswer] {
        traceAnswer.inspect(traceInspector.traceId(), traceInspector.isAnswerTrace());
    };
    QObject::connect(&traceInspector, &ctxfab::TraceInspector::destinationChanged, &traceAnswer,
                     synchronizeTraceAnswer);
    QObject::connect(&traceInspector, &ctxfab::TraceInspector::recordsChanged, &traceAnswer,
                     synchronizeTraceAnswer);
    QObject::connect(&github, &ctxfab::GitHubConnector::repositoryPurged, &traceAnswer,
                     &ctxfab::TraceAnswer::forgetSaved);
    QObject::connect(&app, &QGuiApplication::applicationStateChanged, &traceAnswer,
                     &ctxfab::TraceAnswer::revalidateOnActivation);
    QObject::connect(&github, &ctxfab::GitHubConnector::graphRequested, &graph,
                     &ctxfab::GitHubGraphBrowser::open);
    QQmlApplicationEngine engine;
    configureTheme(engine);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &engine,
                     &QQmlApplicationEngine::retranslate);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &overview,
                     &ctxfab::Overview::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &answers,
                     &ctxfab::Answers::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &answers,
                     &ctxfab::Answers::sessionQuestionsChanged);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &modelLibrary,
                     &ctxfab::ModelLibrary::retranslate);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &localRuntime,
                     &ctxfab::LocalRuntime::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &inferenceSetup,
                     &ctxfab::InferenceSetup::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &embeddingIndex,
                     &ctxfab::EmbeddingIndex::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &preferences,
                     &ctxfab::Preferences::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &preferences,
                     &ctxfab::Preferences::languageChoicesChanged);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &history,
                     &ctxfab::HistoryPreferences::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &answerHistory,
                     &ctxfab::AnswerHistory::retranslate);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &github,
                     &ctxfab::GitHubConnector::retranslate);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &githubActions,
                     &ctxfab::GitHubActionsActivity::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &graph,
                     &ctxfab::GitHubGraphBrowser::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &diagnostics,
                     &ctxfab::Diagnostics::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &diagnostics,
                     &ctxfab::Diagnostics::recordsChanged);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &diagnosticLogs,
                     &ctxfab::DiagnosticLogs::retranslate);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, answerRequests.feed(),
                     &ctxfab::DiagnosticLogs::retranslate);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &answerUsage,
                     &ctxfab::AnswerUsageModel::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &traceInspector,
                     &ctxfab::TraceInspector::changed);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &traceInspector,
                     &ctxfab::TraceInspector::recordsChanged);
    QObject::connect(&appLanguage, &ctxfab::AppLanguage::changed, &traceInspector,
                     &ctxfab::TraceInspector::selectionChanged);
    engine.setInitialProperties(
        {{QStringLiteral("overview"), QVariant::fromValue(&overview)},
         {QStringLiteral("answers"), QVariant::fromValue(&answers)},
         {QStringLiteral("pipelineInspector"), QVariant::fromValue(&pipelineInspector)},
         {QStringLiteral("modelLibrary"), QVariant::fromValue(&modelLibrary)},
         {QStringLiteral("hubCatalog"), QVariant::fromValue(&hubCatalog)},
         {QStringLiteral("inferenceSetup"), QVariant::fromValue(&inferenceSetup)},
         {QStringLiteral("localRuntime"), QVariant::fromValue(&localRuntime)},
         {QStringLiteral("embeddingIndex"), QVariant::fromValue(&embeddingIndex)},
         {QStringLiteral("embeddingProfiles"), QVariant::fromValue(&embeddingProfiles)},
         {QStringLiteral("preferences"), QVariant::fromValue(&preferences)},
         {QStringLiteral("history"), QVariant::fromValue(&history)},
         {QStringLiteral("answerHistory"), QVariant::fromValue(&answerHistory)},
         {QStringLiteral("github"), QVariant::fromValue(&github)},
         {QStringLiteral("githubActions"), QVariant::fromValue(&githubActions)},
         {QStringLiteral("githubGraph"), QVariant::fromValue(&graph)},
         {QStringLiteral("diagnostics"), QVariant::fromValue(&diagnostics)},
         {QStringLiteral("diagnosticLogs"), QVariant::fromValue(&diagnosticLogs)},
         {QStringLiteral("answerRequests"), QVariant::fromValue(&answerRequests)},
         {QStringLiteral("answerUsage"), QVariant::fromValue(&answerUsage)},
         {QStringLiteral("traceInspector"), QVariant::fromValue(&traceInspector)},
         {QStringLiteral("traceAnswer"), QVariant::fromValue(&traceAnswer)},
         {QStringLiteral("appLanguage"), QVariant::fromValue(&appLanguage)}});
    engine.loadFromModule(QStringLiteral("ContextFabricator"), QStringLiteral("Main"));
    if (engine.rootObjects().isEmpty()) {
        return 1;
    }

#ifdef Q_OS_MACOS
    QGuiApplication::setQuitOnLastWindowClosed(false);
    if (auto *window = qobject_cast<QWindow *>(engine.rootObjects().constFirst())) {
        QObject::connect(&app, &QGuiApplication::applicationStateChanged, window,
                         [window](Qt::ApplicationState state) {
                             if (state == Qt::ApplicationActive && !window->isVisible()) {
                                 window->show();
                             }
                         });
    }
#endif

    if (backend.managed()) {
        backend.start();
    } else {
        overview.refresh();
    }
    return QGuiApplication::exec();
}
