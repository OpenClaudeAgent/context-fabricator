#pragma once

#include "GitHubCodeCollection.h"

#include <QCoreApplication>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class GitHubCollection {
    Q_GADGET
    QML_VALUE_TYPE(gitHubCollection)
    Q_DECLARE_TR_FUNCTIONS(GitHubCollection)

    Q_PROPERTY(bool available READ available CONSTANT FINAL)
    Q_PROPERTY(bool canRefresh READ canRefresh CONSTANT FINAL)
    Q_PROPERTY(bool running READ running CONSTANT FINAL)
    Q_PROPERTY(bool graphOnly READ graphOnly CONSTANT FINAL)
    Q_PROPERTY(QString status READ status CONSTANT FINAL)
    Q_PROPERTY(QString pendingCommit READ pendingCommit CONSTANT FINAL)
    Q_PROPERTY(int pendingFiles READ pendingFiles CONSTANT FINAL)
    Q_PROPERTY(QString commit READ commit CONSTANT FINAL)
    Q_PROPERTY(int publishedFiles READ publishedFiles CONSTANT FINAL)
    Q_PROPERTY(QString lastChecked READ lastChecked CONSTANT FINAL)
    Q_PROPERTY(QString nextCheck READ nextCheck CONSTANT FINAL)
    Q_PROPERTY(QString error READ error CONSTANT FINAL)

  public:
    GitHubCollection() = default;
    explicit GitHubCollection(const openapi::GitHubCodeCollection &snapshot,
                              bool graphOnly = false);

    [[nodiscard]] bool available() const { return snapshot_.isValid(); }
    [[nodiscard]] bool canRefresh() const { return available() && snapshot_.isCanRefresh(); }
    [[nodiscard]] bool running() const;
    [[nodiscard]] bool graphOnly() const { return graphOnly_; }
    [[nodiscard]] QString status() const;
    [[nodiscard]] QString pendingCommit() const;
    [[nodiscard]] int pendingFiles() const;
    [[nodiscard]] QString commit() const { return snapshot_.getCommit(); }
    [[nodiscard]] int publishedFiles() const;
    [[nodiscard]] QString lastChecked() const;
    [[nodiscard]] QString nextCheck() const;
    [[nodiscard]] QString error() const;
    [[nodiscard]] int pollDelay(qint64 now) const;

  private:
    openapi::GitHubCodeCollection snapshot_;
    bool graphOnly_ = false;
};

} // namespace ctxfab
