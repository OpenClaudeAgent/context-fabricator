#include "BackendSupervisor.h"

#include <QDir>
#include <QFileInfo>

#include <utility>

namespace ctxfab {

namespace {

constexpr int StartupTimeoutMs = 5000;
constexpr int ShutdownTimeoutMs = 4000;
constexpr int PollIntervalMs = 50;
constexpr int ProfileInUseExitCode = 75;

} // namespace

BackendSupervisor::BackendSupervisor(Options options) : options_(std::move(options)) {
    process_.setStandardOutputFile(QProcess::nullDevice());
    process_.setStandardErrorFile(QProcess::nullDevice());
    process_.setUnixProcessParameters(QProcess::UnixProcessFlag::CloseFileDescriptors);

    poll_.setInterval(PollIntervalMs);
    startDeadline_.setSingleShot(true);
    stopDeadline_.setSingleShot(true);

    connect(&process_, &QProcess::errorOccurred, this, [this](QProcess::ProcessError error) {
        if (error == QProcess::FailedToStart) {
            setState(State::Failed,
                     QT_TR_NOOP("Could not launch the local backend. Check the installation."));
        }
    });
    connect(&process_, &QProcess::finished, this, &BackendSupervisor::childFinished);
    connect(&poll_, &QTimer::timeout, this, &BackendSupervisor::readChildEndpoint);
    connect(&startDeadline_, &QTimer::timeout, this, [this] {
        setState(State::Failed, QT_TR_NOOP("The local backend did not become available in time."));
        process_.closeWriteChannel();
        stopDeadline_.start(ShutdownTimeoutMs);
    });
    connect(&stopDeadline_, &QTimer::timeout, &process_, &QProcess::kill);
}

BackendSupervisor::~BackendSupervisor() {
    disconnect(&process_, nullptr, this, nullptr);
    if (process_.state() == QProcess::NotRunning) {
        return;
    }
    process_.closeWriteChannel();
    if (!process_.waitForFinished(ShutdownTimeoutMs)) {
        process_.kill();
        process_.waitForFinished(1000);
    }
}

QString BackendSupervisor::defaultDataDirectory() {
    const auto configured = qEnvironmentVariable("CTXFAB_DATA_DIRECTORY");
    if (!configured.isEmpty()) {
        return QDir(configured).absolutePath();
    }
#ifdef Q_OS_MACOS
    return QDir::home().filePath(QStringLiteral("Library/Application Support/ContextFabricator"));
#else
    const auto xdgDataHome = qEnvironmentVariable("XDG_DATA_HOME");
    const auto base = QDir::isAbsolutePath(xdgDataHome)
                          ? xdgDataHome
                          : QDir::home().filePath(QStringLiteral(".local/share"));
    return QDir(base).filePath(QStringLiteral("context-fabricator"));
#endif
}

bool BackendSupervisor::canRestart() const {
    return managed() && state_ != State::Starting && state_ != State::Restarting;
}

QString BackendSupervisor::error() const { return error_ ? tr(error_) : QString{}; }

QString BackendSupervisor::profileDirectory() const {
    return managed() ? options_.directory : QFileInfo(options_.externalEndpoint).absolutePath();
}

std::optional<Session> BackendSupervisor::session() const {
    return managed() ? session_ : Session::read(options_.externalEndpoint);
}

void BackendSupervisor::start() {
    if (managed() && state_ == State::Waiting) {
        launch();
    }
}

void BackendSupervisor::restart() {
    if (!canRestart()) {
        return;
    }
    if (process_.state() == QProcess::NotRunning) {
        launch();
        return;
    }
    setState(State::Restarting);
    process_.closeWriteChannel();
    stopDeadline_.start(ShutdownTimeoutMs);
}

void BackendSupervisor::launch() {
    setState(State::Starting);
    startDeadline_.start(StartupTimeoutMs);
    poll_.start();
    process_.start(options_.executable, {options_.directory, QStringLiteral("--app-session")});
}

void BackendSupervisor::readChildEndpoint() {
    if (process_.state() != QProcess::Running) {
        return;
    }
    const auto path = QDir(options_.directory).filePath(QStringLiteral("endpoint.json"));
    auto published = Session::read(path);
    if (published && published->processId == process_.processId()) {
        session_ = std::move(published);
        setState(State::Available);
    }
}

void BackendSupervisor::childFinished(int code, QProcess::ExitStatus exitStatus) {
    stopDeadline_.stop();
    if (state_ == State::Restarting) {
        launch();
        return;
    }
    if (state_ == State::Failed) {
        return;
    }
    const auto detail =
        exitStatus == QProcess::NormalExit && code == ProfileInUseExitCode
            ? QT_TR_NOOP("Another Context Fabricator backend is using this data directory.")
            : QT_TR_NOOP("The local backend exited. Check that its data directory is available.");
    setState(State::Failed, detail);
}

void BackendSupervisor::setState(State state, const char *error) {
    poll_.stop();
    startDeadline_.stop();
    if (state != State::Available) {
        session_.reset();
    }
    state_ = state;
    error_ = error;
    emit changed();
}

} // namespace ctxfab
