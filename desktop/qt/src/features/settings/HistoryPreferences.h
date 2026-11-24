#pragma once

#include "AnswerHistorySettings.h"
#include "LocalApiRequest.h"

#include <QObject>
#include <QtQmlIntegration/qqmlintegration.h>

#include <optional>

namespace ctxfab {

class BackendSupervisor;

class HistoryPreferences final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool conflict READ conflict NOTIFY changed FINAL)
    Q_PROPERTY(bool enabled READ enabled NOTIFY changed FINAL)
    Q_PROPERTY(int retentionDays READ retentionDays NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)

  public:
    explicit HistoryPreferences(BackendSupervisor &backend);

    bool busy() const { return busy_; }
    bool ready() const { return ready_; }
    bool conflict() const { return conflict_; }
    bool enabled() const { return policy_ && policy_->isEnabled(); }
    int retentionDays() const { return policy_ ? policy_->getRetentionDays() : 30; }
    QString feedback() const;

    Q_INVOKABLE void open();
    Q_INVOKABLE void reload();
    Q_INVOKABLE void setEnabled(bool enabled);
    Q_INVOKABLE void setRetentionDays(int days);
    Q_INVOKABLE void setCustomRetention(const QString &value);
    Q_INVOKABLE void clear();

  signals:
    void changed();
    void historyCleared();
    void retentionReduced();

  private:
    openapi::DefaultApi *begin();
    void backendChanged();
    void save(const openapi::AnswerHistorySettings &policy);
    void accept(const openapi::HttpRequestWorker &worker,
                const openapi::AnswerHistorySettings &policy, const QString &message);
    void failed(const openapi::HttpRequestWorker &worker);
    void finish(const QString &message);

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    std::optional<openapi::AnswerHistorySettings> policy_;
    QString feedback_;
    bool opened_ = false;
    bool ready_ = false;
    bool busy_ = false;
    bool conflict_ = false;
};

} // namespace ctxfab
