#include "GitHubGraphOccurrence.h"

#include "GitHubGraphHit.h"
#include "SourceLocation.h"

#include <algorithm>

namespace ctxfab {
namespace {

QString lineFragment(const openapi::SourceLocation &location) {
    const auto start = location.getStartLine();
    const auto end = location.getEndLine();
    if (start < 1 || end < start) {
        return {};
    }
    const auto first = QStringLiteral("L%1").arg(start);
    return start == end ? first : QStringLiteral("%1-L%2").arg(first).arg(end);
}

QString locationText(const openapi::SourceLocation &location, const QString &fragment) {
    return fragment.isEmpty() ? location.getPath()
                              : QStringLiteral("%1#%2").arg(location.getPath(), fragment);
}

QUrl pinnedUrl(const openapi::SourceLocation &location, const QString &prefix,
               const QString &fragment) {
    const auto path = location.getPath();
    const auto segments = path.split('/');
    const bool safePath = std::ranges::none_of(segments, [](const auto &segment) {
        return segment.isEmpty() || segment == "." || segment == "..";
    });
    QUrl url(location.getFileUrl(), QUrl::StrictMode);
    const auto expected =
        "https://github.com" + prefix.toUtf8() + QUrl::toPercentEncoding(path, "/");
    if (!safePath || fragment.isEmpty() || url.toEncoded() != expected) {
        return {};
    }
    url.setFragment(fragment);
    return url;
}

} // namespace

GitHubGraphOccurrence::GitHubGraphOccurrence(const openapi::GitHubGraphHit &hit,
                                             const QString &repository, const QString &revision)
    : name_(hit.is_qualified_name_Set() ? hit.getQualifiedName() : hit.getName()),
      role_(hit.getRole()), resolution_(hit.getResolution()) {
    const auto prefix = QStringLiteral("/%1/blob/%2/").arg(repository, revision);
    const auto site = hit.getSite();
    const auto siteLines = lineFragment(site);
    sourceText_ = locationText(site, siteLines);
    sourceUrl_ = pinnedUrl(site, prefix, siteLines);
    if (hit.is_target_Set()) {
        const auto target = hit.getTarget();
        const auto targetLines = lineFragment(target);
        targetText_ = locationText(target, targetLines);
        targetUrl_ = pinnedUrl(target, prefix, targetLines);
    }
}

} // namespace ctxfab
