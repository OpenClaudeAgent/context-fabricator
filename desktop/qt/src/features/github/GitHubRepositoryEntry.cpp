#include "GitHubRepositoryEntry.h"

#include "GitHubCollection.h"

namespace ctxfab {

GitHubRepositoryEntry::GitHubRepositoryEntry(const openapi::GitHubRepository &repository)
    : id_(repository.getId()), name_(repository.getFullName()),
      branch_(repository.getDefaultBranch()), privateRepository_(repository.isRPrivate()) {}

GitHubRepositoryEntry::GitHubRepositoryEntry(const openapi::GitHubConfiguredRepository &repository)
    : GitHubRepositoryEntry(repository.getRepository()) {
    branch_ = repository.getBranch();
    collectionStatus_ = GitHubCollection(repository.getCodeCollection()).status();
    accessWarning_ = warningFor(repository);
}

QString GitHubRepositoryEntry::warningFor(const openapi::GitHubConfiguredRepository &repository) {
    using Access = openapi::GitHubAccess::eGitHubAccess;
    if (!repository.isVerified()) {
        return tr("Verify access");
    }
    const bool actionsUnavailable = repository.getActions().getValue() == Access::UNAVAILABLE;
    const bool contentsUnavailable = repository.getContents().getValue() == Access::UNAVAILABLE;
    if (actionsUnavailable && contentsUnavailable) {
        return tr("Access unverified");
    }
    if (actionsUnavailable) {
        return tr("Actions access unverified");
    }
    if (contentsUnavailable) {
        return tr("Contents access unverified");
    }
    return {};
}

} // namespace ctxfab
