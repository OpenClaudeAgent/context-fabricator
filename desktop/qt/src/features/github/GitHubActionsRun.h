#pragma once

#include "GitHubActionRun.h"

#include <QCoreApplication>
#include <QUrl>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class GitHubActionsRun {
    Q_GADGET
    QML_VALUE_TYPE(gitHubActionsRun)
    Q_DECLARE_TR_FUNCTIONS(GitHubActionsRun)

    Q_PROPERTY(QString workflow READ workflow CONSTANT FINAL)
    Q_PROPERTY(QString id READ id CONSTANT FINAL)
    Q_PROPERTY(qint64 attempt READ attempt CONSTANT FINAL)
    Q_PROPERTY(QString result READ result CONSTANT FINAL)
    Q_PROPERTY(QString resultLabel READ resultLabel CONSTANT FINAL)
    Q_PROPERTY(QString summary READ summary CONSTANT FINAL)
    Q_PROPERTY(QString createdAt READ createdAt CONSTANT FINAL)
    Q_PROPERTY(QString linkLabel READ linkLabel CONSTANT FINAL)
    Q_PROPERTY(QUrl url READ url CONSTANT FINAL)

  public:
    GitHubActionsRun() = default;
    GitHubActionsRun(const openapi::GitHubActionRun &run, const QString &workflow,
                     const QString &repository);

    [[nodiscard]] QString workflow() const;
    [[nodiscard]] qint64 workflowId() const { return run_.getWorkflowId(); }
    [[nodiscard]] QString id() const { return QString::number(run_.getId()); }
    [[nodiscard]] qint64 attempt() const { return run_.getAttempt(); }
    [[nodiscard]] QString result() const;
    [[nodiscard]] QString resultLabel() const;
    [[nodiscard]] QString summary() const;
    [[nodiscard]] QString createdAt() const;
    [[nodiscard]] QString linkLabel() const;
    [[nodiscard]] QUrl url() const { return url_; }

  private:
    openapi::GitHubActionRun run_;
    QString workflow_;
    QUrl url_;
};

} // namespace ctxfab
