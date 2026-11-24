#pragma once

#include "GitHubConfiguredRepository.h"
#include "GitHubRepository.h"

#include <QCoreApplication>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class GitHubRepositoryEntry {
    Q_GADGET
    QML_VALUE_TYPE(gitHubRepositoryEntry)
    Q_DECLARE_TR_FUNCTIONS(GitHubRepositoryEntry)

    Q_PROPERTY(QString name READ name CONSTANT FINAL)
    Q_PROPERTY(QString branch READ branch CONSTANT FINAL)
    Q_PROPERTY(bool privateRepository READ privateRepository CONSTANT FINAL)
    Q_PROPERTY(QString collectionStatus READ collectionStatus CONSTANT FINAL)
    Q_PROPERTY(QString accessWarning READ accessWarning CONSTANT FINAL)

  public:
    GitHubRepositoryEntry() = default;
    explicit GitHubRepositoryEntry(const openapi::GitHubRepository &repository);
    explicit GitHubRepositoryEntry(const openapi::GitHubConfiguredRepository &repository);

    [[nodiscard]] qint64 id() const { return id_; }
    [[nodiscard]] QString name() const { return name_; }
    [[nodiscard]] QString branch() const { return branch_; }
    [[nodiscard]] bool privateRepository() const { return privateRepository_; }
    [[nodiscard]] QString collectionStatus() const { return collectionStatus_; }
    [[nodiscard]] QString accessWarning() const { return accessWarning_; }
    bool operator==(const GitHubRepositoryEntry &) const = default;

  private:
    static QString warningFor(const openapi::GitHubConfiguredRepository &repository);

    qint64 id_ = 0;
    QString name_;
    QString branch_;
    QString collectionStatus_;
    QString accessWarning_;
    bool privateRepository_ = false;
};

} // namespace ctxfab
