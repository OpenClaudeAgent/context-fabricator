#include "BenchmarkSources.h"

#include "DefaultApi.h"

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;

QString connectedKey(const openapi::GitHubActions &snapshot, const openapi::GitHubActionRun &run) {
    return QStringLiteral("connected/%1/%2/%3/%4")
        .arg(snapshot.getRepositoryId())
        .arg(snapshot.getBranch())
        .arg(run.getId())
        .arg(run.getAttempt());
}

QString workflowTitle(const openapi::GitHubActions &snapshot, const openapi::GitHubActionRun &run) {
    for (const auto &workflow : snapshot.getWorkflows()) {
        if (workflow.getId() == run.getWorkflowId()) {
            return snapshot.getRepository() + QStringLiteral(" · ") + workflow.getName();
        }
    }
    return snapshot.getRepository();
}

QString workflowDate(const openapi::GitHubActions &snapshot, const openapi::GitHubActionRun &run) {
    return snapshot.getBranch() + QStringLiteral(" · ") +
           run.getCreatedAt().toLocalTime().toString(Qt::ISODate);
}

} // namespace

QList<BenchmarkWorkflowRow> BenchmarkSources::connectedWorkflows() const {
    QList<BenchmarkWorkflowRow> rows;
    for (const auto &snapshot : connected_) {
        for (const auto &run : snapshot.getRuns()) {
            const auto id = connectedKey(snapshot, run);
            rows.append({id, workflowTitle(snapshot, run), workflowDate(snapshot, run),
                         !inactive_.contains(id), false});
        }
    }
    return rows;
}

QList<BenchmarkSuite> BenchmarkSources::connectedSuites() const {
    QList<BenchmarkSuite> suites;
    for (const auto &configuration : repositories_) {
        const auto repository = configuration.getRepository();
        openapi::Question scope;
        scope.setRepositoryId(repository.getId());
        scope.setBranch(configuration.getBranch());
        suites.append(
            {.id = QStringLiteral("repository/%1").arg(repository.getId()),
             .title = repository.getFullName() + QStringLiteral(" · ") + scope.getBranch(),
             .questions = {},
             .ready = true,
             .localScope = scope});
    }
    for (const auto &snapshot : connected_) {
        for (const auto &run : snapshot.getRuns()) {
            const auto id = connectedKey(snapshot, run);
            if (inactive_.contains(id)) {
                continue;
            }
            openapi::Question scope;
            scope.setRepositoryId(snapshot.getRepositoryId());
            scope.setBranch(snapshot.getBranch());
            scope.setActionsRunId(run.getId());
            scope.setActionsRunAttempt(run.getAttempt());
            suites.append({.id = id,
                           .title = workflowTitle(snapshot, run) + QStringLiteral(" · ") +
                                    workflowDate(snapshot, run),
                           .questions = {},
                           .ready = true,
                           .localScope = scope});
        }
    }
    return suites;
}

void BenchmarkSources::loadConnectedSources() {
    if (busy_) {
        return;
    }
    auto *api = begin();
    if (!api) {
        finish(tr("Backend request failed"));
        return;
    }
    busy_ = true;
    feedback_.clear();
    emit changed();
    connect(api, &Api::readGitHubStateSignalErrorFull, this, &BenchmarkSources::failure);
    connect(api, &Api::readGitHubStateSignalFull, this,
            [this](const Worker *, const openapi::GitHubState &state) {
                repositories_ = state.getRepositories();
                connected_.clear();
                for (const auto &configuration : repositories_) {
                    if (configuration.getFeatures().isActions()) {
                        pendingConnected_.enqueue(configuration.getRepository().getId());
                    }
                }
                readNextConnected();
            });
    api->readGitHubState();
}

void BenchmarkSources::readNextConnected() {
    if (pendingConnected_.isEmpty()) {
        connectedLoaded_ = feedback_.isEmpty();
        finish(feedback_);
        return;
    }
    auto *api = begin();
    if (!api) {
        pendingConnected_.clear();
        finish(tr("Backend request failed"));
        return;
    }
    connect(api, &Api::readGitHubActionsSignalFull, this,
            [this](const Worker *, const openapi::GitHubActionsState &state) {
                const auto snapshot = state.getSnapshot();
                if (state.getAvailability() == QStringLiteral("ready") && snapshot.isValid()) {
                    connected_.append(snapshot);
                }
                readNextConnected();
            });
    connect(api, &Api::readGitHubActionsSignalErrorFull, this, [this](const Worker *worker) {
        feedback_ = LocalApiRequest::rejection(*worker, tr("Backend request failed"));
        readNextConnected();
    });
    api->readGitHubActions(pendingConnected_.dequeue());
}

} // namespace ctxfab
