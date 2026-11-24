#include "GitHubActionsActivity.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "GitHubActionWorkflow.h"

#include <QLocale>

#include <algorithm>
#include <iterator>
#include <tuple>

namespace ctxfab {
namespace {
using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
using Activity = GitHubActionsActivity;

Activity::Availability readAvailability(const QString &value) {
    if (value == "ready") {
        return Activity::Ready;
    }
    if (value == "disabled") {
        return Activity::Disabled;
    }
    if (value == "reconnectRequired") {
        return Activity::ReconnectRequired;
    }
    return value == "accessRequired" ? Activity::AccessRequired : Activity::Unknown;
}

auto runOrder(const openapi::GitHubActionRun &run) {
    return std::tuple(run.getCreatedAt(), run.getId(), run.getAttempt());
}
} // namespace

GitHubActionsActivity::GitHubActionsActivity(BackendSupervisor &backend, GitHubConnector &connector)
    : backend_(backend), connector_(connector) {
    connect(&connector_, &GitHubConnector::changed, this, &GitHubActionsActivity::synchronize);
    synchronize();
}

QString GitHubActionsActivity::nextCheck() const {
    if (!state_.is_schedule_Set()) {
        return {};
    }
    const auto next = state_.getSchedule().getNextAtMs();
    return next == 0
               ? tr("Due now")
               : QLocale().toString(QDateTime::fromMSecsSinceEpoch(next), QLocale::ShortFormat);
}

QString GitHubActionsActivity::feedback() const { return tr(qUtf8Printable(feedback_)); }

QStringList GitHubActionsActivity::workflows() const {
    QStringList labels{tr("All Workflows")};
    const auto workflows = snapshot().getWorkflows();
    for (const auto &workflow : workflows) {
        labels.append(workflow.getName());
    }
    return labels;
}

int GitHubActionsActivity::workflowIndex() const {
    const auto workflows = snapshot().getWorkflows();
    const auto selected =
        std::ranges::find(workflows, workflowId_, &openapi::GitHubActionWorkflow::getId);
    return selected == workflows.end()
               ? 0
               : static_cast<int>(std::distance(workflows.begin(), selected)) + 1;
}

QList<GitHubActionsRun> GitHubActionsActivity::runs() const {
    if (workflowId_ == 0) {
        return runs_;
    }
    QList<GitHubActionsRun> selected;
    std::ranges::copy_if(runs_, std::back_inserter(selected),
                         [this](const auto &run) { return run.workflowId() == workflowId_; });
    return selected;
}

void GitHubActionsActivity::selectWorkflow(int index) {
    const auto workflows = snapshot().getWorkflows();
    if (index < 0 || index > workflows.size()) {
        return;
    }
    const auto selection = index == 0 ? 0 : workflows.at(index - 1).getId();
    if (selection != workflowId_) {
        workflowId_ = selection;
        emit changed();
    }
}

void GitHubActionsActivity::show() {
    if (active() && !visible_) {
        visible_ = true;
        workflowId_ = 0;
        emit changed();
        reload();
    }
}

void GitHubActionsActivity::close() {
    if (visible_) {
        visible_ = false;
        emit changed();
    }
}

void GitHubActionsActivity::synchronize() {
    const auto repository = connector_.configuredDraft();
    if (!repository || !repository->getFeatures().isActions()) {
        if (active()) {
            reset();
            emit changed();
        }
        return;
    }
    const auto identity = repository->getRepository();
    const Scope scope{identity.getId(), identity.getFullName(), repository->getBranch()};
    const Access access{connector_.credential(), repository->getContents().getValue(),
                        repository->getActions().getValue(), repository->isVerified()};
    const bool paused = connector_.draftDataPaused();
    const bool purged = paused && !paused_;
    paused_ = paused;
    if (scope == scope_ && access == access_ && !purged) {
        return;
    }
    if (scope != scope_) {
        reset();
        scope_ = scope;
        paused_ = paused;
    }
    access_ = access;
    busy_ = false;
    reload();
}

void GitHubActionsActivity::reset() {
    request_.cancel();
    scope_ = {};
    access_ = {};
    state_ = {};
    runs_.clear();
    workflowId_ = 0;
    feedback_.clear();
    availability_ = Unknown;
    visible_ = false;
    busy_ = false;
    paused_ = false;
}

Api *GitHubActionsActivity::begin() {
    if (!active() || busy_) {
        return nullptr;
    }
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Backend session unavailable"));
        return nullptr;
    }
    busy_ = true;
    feedback_.clear();
    return request_.begin(*session);
}

void GitHubActionsActivity::reload() { read(); }

void GitHubActionsActivity::read(bool refreshed, const QString &refreshError) {
    if (auto *api = begin()) {
        connect(api, &Api::readGitHubActionsSignalFull, this,
                [this, refreshed, refreshError](const Worker *worker,
                                                const openapi::GitHubActionsState &state) {
                    receive(*worker, state, refreshed, refreshError);
                });
        connect(api, &Api::readGitHubActionsSignalErrorFull, this,
                [this, refreshError](const Worker *worker) {
                    finish(refreshError.isEmpty()
                               ? LocalApiRequest::rejection(*worker,
                                                            QT_TR_NOOP("Backend request failed"))
                               : refreshError);
                });
        api->readGitHubActions(scope_.id);
        emit changed();
    }
}

void GitHubActionsActivity::refresh() {
    if (availability_ != Ready || connector_.busy()) {
        return;
    }
    if (auto *api = begin()) {
        connect(api, &Api::refreshGitHubActionsSignalFull, this,
                [this](const Worker *worker, const openapi::GitHubActions &snapshot) {
                    if (worker->getHttpResponseCode() == 200 && matchesScope(snapshot)) {
                        finishRefresh(true);
                    } else {
                        finishRefresh(false, QT_TR_NOOP("Invalid response from the backend"));
                    }
                });
        connect(api, &Api::refreshGitHubActionsSignalErrorFull, this, [this](const Worker *worker) {
            finishRefresh(
                false, LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
        });
        api->refreshGitHubActions(scope_.id);
        emit changed();
    }
}

void GitHubActionsActivity::finishRefresh(bool success, const QString &error) {
    request_.cancel();
    busy_ = false;
    read(success, error);
    connector_.reload();
}

bool GitHubActionsActivity::matchesScope(const openapi::GitHubActions &snapshot) const {
    return snapshot.isValid() && snapshot.getRepositoryId() == scope_.id &&
           snapshot.getRepository() == scope_.repository && snapshot.getBranch() == scope_.branch;
}

openapi::GitHubActions GitHubActionsActivity::snapshot() const {
    return hasSnapshot() ? state_.getSnapshot() : openapi::GitHubActions{};
}

void GitHubActionsActivity::receive(const Worker &worker, const openapi::GitHubActionsState &state,
                                    bool refreshed, const QString &refreshError) {
    if (worker.getHttpResponseCode() != 200 || !state.isValid() ||
        readAvailability(state.getAvailability()) == Unknown ||
        (state.is_snapshot_Set() && !matchesScope(state.getSnapshot()))) {
        finish(refreshError.isEmpty() ? QT_TR_NOOP("Invalid response from the backend")
                                      : refreshError);
        return;
    }
    publish(state);
    finish(refreshed ? QT_TR_NOOP("Workflow runs refreshed.") : refreshError);
}

void GitHubActionsActivity::publish(const openapi::GitHubActionsState &state) {
    state_ = state;
    availability_ = readAvailability(state.getAvailability());
    if (workflowIndex() == 0) {
        workflowId_ = 0;
    }
    const auto current = snapshot();
    QHash<qint64, QString> names;
    const auto workflows = current.getWorkflows();
    for (const auto &workflow : workflows) {
        names.insert(workflow.getId(), workflow.getName());
    }
    auto observations = current.getRuns();
    std::ranges::sort(observations, std::ranges::greater{}, runOrder);
    runs_.clear();
    for (const auto &run : observations) {
        runs_.append(
            GitHubActionsRun(run, names.value(run.getWorkflowId()), current.getRepository()));
    }
}

void GitHubActionsActivity::finish(const QString &feedback) {
    request_.cancel();
    busy_ = false;
    feedback_ = feedback;
    emit changed();
}

} // namespace ctxfab
