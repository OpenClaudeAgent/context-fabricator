#include "GitHubGraphBrowser.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "GitHubGraphRoot.h"

#include <QRegularExpression>

namespace ctxfab {
namespace {

using Browser = GitHubGraphBrowser;
using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;

Browser::Access graphAccess(const QString &value) {
    if (value == "available") {
        return Browser::Available;
    }
    if (value == "retained") {
        return Browser::Retained;
    }
    return value == "unavailable" ? Browser::Unavailable : Browser::Unknown;
}

Browser::Coverage graphCoverage(const QString &value) {
    if (value == "published") {
        return Browser::Published;
    }
    if (value == "noConventionalRustRoots") {
        return Browser::NoConventionalRustRoots;
    }
    return value == "limit" ? Browser::Limit : Browser::NoSnapshot;
}

bool validSnapshot(const openapi::GitHubGraph &graph) {
    if (!graph.is_snapshot_Set()) {
        return true;
    }
    const auto snapshot = graph.getSnapshot();
    static const QRegularExpression revision(QStringLiteral("\\A[a-f0-9]{40}\\z"));
    return revision.match(snapshot.getRevision()).hasMatch() &&
           graphCoverage(snapshot.getCoverage()) != Browser::NoSnapshot;
}

} // namespace

GitHubGraphBrowser::GitHubGraphBrowser(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &GitHubGraphBrowser::close);
}

GitHubGraphBrowser::Coverage GitHubGraphBrowser::coverage() const {
    return graphCoverage(snapshot_.getCoverage());
}

QString GitHubGraphBrowser::feedback() const { return tr(qUtf8Printable(feedback_)); }

QStringList GitHubGraphBrowser::roots() const {
    QStringList paths;
    const auto roots = snapshot_.getRoots();
    for (const auto &root : roots) {
        paths.append(root.getPath());
    }
    return paths;
}

int GitHubGraphBrowser::rootIndex() const {
    return static_cast<int>(roots().indexOf(selectedRoot_));
}

QString GitHubGraphBrowser::rootSummary() const {
    const auto index = rootIndex();
    if (index < 0) {
        return {};
    }
    const auto root = snapshot_.getRoots().at(index);
    return tr("%1 reachable files · %2 parse errors · %3 limited")
        .arg(root.getReachableFiles())
        .arg(root.getParseErrors())
        .arg(root.getLimitedFiles());
}

void GitHubGraphBrowser::open(qint64 id, const QString &repository, const QString &branch) {
    if (id <= 0 || repository.isEmpty() || branch.isEmpty()) {
        return;
    }
    reset();
    id_ = id;
    repository_ = repository;
    branch_ = branch;
    load({});
}

void GitHubGraphBrowser::close() {
    if (visible()) {
        reset();
        emit changed();
    }
}

void GitHubGraphBrowser::reset() {
    request_.cancel();
    id_ = 0;
    repository_.clear();
    branch_.clear();
    busy_ = false;
    access_ = Unknown;
    snapshot_ = {};
    selectedRoot_.clear();
    search_ = {};
    hits_.clear();
    feedback_.clear();
}

void GitHubGraphBrowser::reload() { load(search_); }

void GitHubGraphBrowser::selectRoot(int index) {
    const auto paths = roots();
    if (!busy_ && index >= 0 && index < paths.size() && paths.at(index) != selectedRoot_) {
        selectedRoot_ = paths.at(index);
        emit changed();
    }
}

void GitHubGraphBrowser::lookup(const QString &symbol) {
    const auto trimmed = symbol.trimmed();
    if (!selectedRoot_.isEmpty() && !trimmed.isEmpty()) {
        load({selectedRoot_, trimmed});
    }
}

void GitHubGraphBrowser::load(const Search &search) {
    if (!visible() || busy_) {
        return;
    }
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    auto *api = request_.begin(*session);
    busy_ = true;
    feedback_.clear();
    connect(api, &Api::readGitHubGraphSignalFull, this,
            [this, search](const Worker *worker, const openapi::GitHubGraph &graph) {
                receive(*worker, graph, search);
            });
    connect(api, &Api::readGitHubGraphSignalErrorFull, this, [this](const Worker *worker) {
        finish(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    if (search.symbol.isEmpty()) {
        api->readGitHubGraph(id_, branch_);
    } else {
        api->readGitHubGraph(id_, branch_, search.root, search.symbol);
    }
    emit changed();
}

void GitHubGraphBrowser::receive(const Worker &worker, const openapi::GitHubGraph &graph,
                                 const Search &search) {
    if (worker.getHttpResponseCode() != 200 || !graph.isValid() || graph.getRepositoryId() != id_ ||
        graph.getBranch() != branch_ || graphAccess(graph.getAccess()) == Unknown ||
        !validSnapshot(graph)) {
        finish(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    publish(graph, search);
    finish();
}

void GitHubGraphBrowser::publish(const openapi::GitHubGraph &graph, const Search &search) {
    access_ = graphAccess(graph.getAccess());
    snapshot_ = graph.getSnapshot();
    const auto paths = roots();
    if (!paths.contains(selectedRoot_)) {
        selectedRoot_ = paths.value(0);
    }
    search_ = coverage() == Published ? search : Search{};
    hits_.clear();
    const auto hits = snapshot_.getHits();
    for (const auto &hit : hits) {
        hits_.append(GitHubGraphOccurrence(hit, repository_, snapshot_.getRevision()));
    }
}

void GitHubGraphBrowser::finish(const QString &feedback) {
    busy_ = false;
    feedback_ = feedback;
    request_.cancel();
    emit changed();
}

} // namespace ctxfab
