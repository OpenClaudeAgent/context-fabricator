#pragma once

#include "GitHubActionsRun.h"
#include "GitHubActionsState.h"
#include "GitHubConnector.h"
#include "LocalApiRequest.h"

#include <QObject>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class GitHubActionsActivity final : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(GitHubActionsActivity)
    QML_UNCREATABLE("Created by the desktop application")

    Q_PROPERTY(bool active READ active NOTIFY changed FINAL)
    Q_PROPERTY(bool visible READ visible NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(Availability availability READ availability NOTIFY changed FINAL)
    Q_PROPERTY(bool hasSnapshot READ hasSnapshot NOTIFY changed FINAL)
    Q_PROPERTY(QString repository READ repository NOTIFY changed FINAL)
    Q_PROPERTY(QString branch READ branch NOTIFY changed FINAL)
    Q_PROPERTY(QString nextCheck READ nextCheck NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QStringList workflows READ workflows NOTIFY changed FINAL)
    Q_PROPERTY(int workflowIndex READ workflowIndex NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::GitHubActionsRun> runs READ runs NOTIFY changed FINAL)

  public:
    enum Availability { Unknown, Ready, Disabled, ReconnectRequired, AccessRequired };
    Q_ENUM(Availability)

    GitHubActionsActivity(BackendSupervisor &backend, GitHubConnector &connector);

    [[nodiscard]] bool active() const { return scope_.id > 0; }
    [[nodiscard]] bool visible() const { return visible_; }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] Availability availability() const { return availability_; }
    [[nodiscard]] bool hasSnapshot() const { return state_.is_snapshot_Set(); }
    [[nodiscard]] QString repository() const { return scope_.repository; }
    [[nodiscard]] QString branch() const { return scope_.branch; }
    [[nodiscard]] QString nextCheck() const;
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] QStringList workflows() const;
    [[nodiscard]] int workflowIndex() const;
    [[nodiscard]] QList<GitHubActionsRun> runs() const;

    Q_INVOKABLE void show();
    Q_INVOKABLE void close();
    Q_INVOKABLE void reload();
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void selectWorkflow(int index);

  signals:
    void changed();

  private:
    struct Scope {
        qint64 id = 0;
        QString repository;
        QString branch;
        bool operator==(const Scope &) const = default;
    };

    using Permission = openapi::GitHubAccess::eGitHubAccess;
    struct Access {
        GitHubConnector::Credential credential = GitHubConnector::Unknown;
        Permission contents = Permission::INVALID_VALUE_OPENAPI_GENERATED;
        Permission actions = Permission::INVALID_VALUE_OPENAPI_GENERATED;
        bool verified = false;
        bool operator==(const Access &) const = default;
    };

    void synchronize();
    void reset();
    openapi::DefaultApi *begin();
    void read(bool refreshed = false, const QString &refreshError = {});
    void finishRefresh(bool success, const QString &error = {});
    void receive(const openapi::HttpRequestWorker &worker, const openapi::GitHubActionsState &state,
                 bool refreshed, const QString &refreshError);
    bool matchesScope(const openapi::GitHubActions &snapshot) const;
    [[nodiscard]] openapi::GitHubActions snapshot() const;
    void publish(const openapi::GitHubActionsState &state);
    void finish(const QString &feedback = {});

    BackendSupervisor &backend_;
    GitHubConnector &connector_;
    LocalApiRequest request_{90000};
    Scope scope_;
    Access access_;
    openapi::GitHubActionsState state_;
    QList<GitHubActionsRun> runs_;
    qint64 workflowId_ = 0;
    QString feedback_;
    Availability availability_ = Unknown;
    bool visible_ = false;
    bool busy_ = false;
    bool paused_ = false;
};

} // namespace ctxfab
