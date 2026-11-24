#include "LocalRuntime.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "LibraryModel.h"

#include <QDir>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
constexpr int ArchiveTimeoutMs = 120'000;
constexpr int PollIntervalMs = 1000;

bool validStatus(const openapi::RuntimeStatus &status) {
    static const QStringList states{"unavailable", "availableForInstall", "installing", "ready",
                                    "failed"};
    return status.isValid() && states.contains(status.getState());
}

} // namespace

LocalRuntime::LocalRuntime(BackendSupervisor &backend) : backend_(backend) {
    poll_.setInterval(PollIntervalMs);
    connect(&poll_, &QTimer::timeout, this, &LocalRuntime::refresh);
    connect(&backend_, &BackendSupervisor::changed, this, &LocalRuntime::reset);
}

bool LocalRuntime::hasRuntime() const {
    return runtime_.isValid() && runtime_.getState() != QStringLiteral("unavailable");
}

bool LocalRuntime::issuing() const {
    return pending_ == Request::Download || pending_ == Request::Import;
}

bool LocalRuntime::busy() const {
    return issuing() || runtime_.getState() == QStringLiteral("installing");
}

bool LocalRuntime::canInstall() const {
    const auto state = runtime_.getState();
    return !issuing() &&
           (state == QStringLiteral("availableForInstall") || state == QStringLiteral("failed"));
}

bool LocalRuntime::canDownload() const { return canInstall() && runtime_.isCanDownload(); }

bool LocalRuntime::canImport() const { return canInstall() && runtime_.getArchiveBytes() > 0; }

bool LocalRuntime::needsRestart() const {
    return runtime_.getState() != QStringLiteral("ready") || runtime_.getArchiveBytes() > 0;
}

QString LocalRuntime::engine() const {
    return runtime_.is_engine_Set() ? runtime_.getEngine().getName() : tr("Inference engine");
}

QString LocalRuntime::status() const {
    if (busy()) {
        return tr("Installing…");
    }
    if (runtime_.getState() != QStringLiteral("ready")) {
        return {};
    }
    return runtime_.is_engine_Set() ? tr("Build %1").arg(runtime_.getEngine().getBuild())
                                    : tr("Ready");
}

QString LocalRuntime::archiveSummary() const {
    const auto bytes = runtime_.getArchiveBytes();
    return bytes > 0 ? tr("Runtime archive: %1").arg(formatArtifactSize(bytes)) : QString{};
}

QString LocalRuntime::feedback() const {
    if (!commandError_.isEmpty()) {
        return commandError_;
    }
    return !readError_.isEmpty() ? readError_ : runtime_.getError();
}

void LocalRuntime::setObserved(bool observed) {
    if (observed) {
        poll_.start();
        refresh();
    } else {
        poll_.stop();
        if (pending_ == Request::Observe) {
            request_.cancel();
            pending_ = Request::None;
        }
    }
}

void LocalRuntime::reset() {
    request_.cancel();
    pending_ = Request::None;
    runtime_ = {};
    readError_.clear();
    commandError_.clear();
    emit changed();
    refresh();
}

void LocalRuntime::refresh() {
    if (poll_.isActive() && pending_ == Request::None) {
        send(Request::Observe);
    }
}

void LocalRuntime::download() {
    if (poll_.isActive() && canDownload()) {
        send(Request::Download);
    }
}

void LocalRuntime::install(const QUrl &archive) {
    if (!poll_.isActive() || !canImport()) {
        return;
    }
    const auto path = archive.toLocalFile();
    if (!QDir::isAbsolutePath(path) || path.toUcs4().size() > 2048 || path.contains(QChar::Null)) {
        commandError_ = tr("Choose a local runtime archive.");
        emit changed();
        return;
    }
    send(Request::Import, path);
}

void LocalRuntime::send(Request request, const QString &archivePath) {
    const auto session = backend_.session();
    if (!session) {
        return;
    }
    auto *api = request_.begin(
        *session, request == Request::Import ? std::optional{ArchiveTimeoutMs} : std::nullopt);
    pending_ = request;
    if (issuing()) {
        commandError_.clear();
        emit changed();
    }
    const auto success = [this](const Worker *worker, const openapi::RuntimeStatus &snapshot) {
        receive(*worker, snapshot);
    };
    const auto failure = [this](const Worker *worker) { reject(*worker); };
    switch (request) {
    case Request::Download:
        connect(api, &Api::downloadRuntimeSignalFull, this, success);
        connect(api, &Api::downloadRuntimeSignalErrorFull, this, failure);
        api->downloadRuntime();
        break;
    case Request::Import: {
        connect(api, &Api::installRuntimeSignalFull, this, success);
        connect(api, &Api::installRuntimeSignalErrorFull, this, failure);
        openapi::InstallRuntime_request body;
        body.setArchivePath(archivePath);
        api->installRuntime(body);
        break;
    }
    case Request::Observe:
        connect(api, &Api::readRuntimeStatusSignalFull, this, success);
        connect(api, &Api::readRuntimeStatusSignalErrorFull, this, failure);
        api->readRuntimeStatus();
        break;
    case Request::None:
        break;
    }
}

void LocalRuntime::receive(const Worker &worker, const openapi::RuntimeStatus &status) {
    if (worker.getHttpResponseCode() != 200 || !validStatus(status)) {
        reject(worker);
        return;
    }
    request_.cancel();
    pending_ = Request::None;
    runtime_ = status;
    readError_.clear();
    emit changed();
}

void LocalRuntime::reject(const Worker &worker) {
    if (issuing()) {
        commandError_ =
            LocalApiRequest::rejection(worker, tr("Could not update the local runtime."));
    } else {
        readError_ = LocalApiRequest::rejection(worker, tr("Could not read the local runtime."));
    }
    request_.cancel();
    pending_ = Request::None;
    emit changed();
}

} // namespace ctxfab
