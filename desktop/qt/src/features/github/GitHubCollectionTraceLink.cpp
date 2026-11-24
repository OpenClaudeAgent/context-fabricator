#include "GitHubCollectionTraceLink.h"

namespace ctxfab {

GitHubCollectionTraceLink::GitHubCollectionTraceLink(const openapi::GitHubCollectionTrace &trace,
                                                     Kind kind)
    : trace_(trace), kind_(kind) {}

QString GitHubCollectionTraceLink::label() const {
    switch (kind_) {
    case Kind::Collection:
        return tr("View collection trace");
    case Kind::Latest:
        return tr("View latest attempt trace");
    case Kind::Published:
        return tr("View published snapshot trace");
    }
    return {};
}

QString GitHubCollectionTraceLink::kind() const {
    switch (kind_) {
    case Kind::Collection:
        return QStringLiteral("collection");
    case Kind::Latest:
        return QStringLiteral("latest");
    case Kind::Published:
        return QStringLiteral("published");
    }
    return {};
}

QList<GitHubCollectionTraceLink>
GitHubCollectionTraceLink::links(const openapi::GitHubCollectionActivity &activity) {
    const auto latest = activity.getLatestAttempt();
    const auto published = activity.getPublishedSnapshot();
    if (latest.isValid() && published.isValid() && latest.getTraceId() == published.getTraceId() &&
        latest.getSpanId() == published.getSpanId()) {
        return {GitHubCollectionTraceLink(latest, Kind::Collection)};
    }
    QList<GitHubCollectionTraceLink> result;
    if (latest.isValid()) {
        result.append(GitHubCollectionTraceLink(latest, Kind::Latest));
    }
    if (published.isValid()) {
        result.append(GitHubCollectionTraceLink(published, Kind::Published));
    }
    return result;
}

} // namespace ctxfab
