#pragma once

#include "LocalApiRequest.h"
#include "SavedAnswer.h"
#include "TraceAnswerDetails.h"

#include <QObject>
#include <QTimer>
#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

class Answers;
class BackendSupervisor;
class HistoryPreferences;

class TraceAnswer final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")

    Q_PROPERTY(bool available READ available NOTIFY changed FINAL)
    Q_PROPERTY(bool live READ live NOTIFY changed FINAL)
    Q_PROPERTY(bool loading READ loading NOTIFY changed FINAL)
    Q_PROPERTY(QString question READ question NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::AnswerResult result READ result NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::TraceAnswerDetails details READ details NOTIFY changed FINAL)

  public:
    TraceAnswer(BackendSupervisor &backend, Answers &answers, HistoryPreferences &preferences);

    [[nodiscard]] bool available() const;
    [[nodiscard]] bool live() const;
    [[nodiscard]] bool loading() const { return loading_; }
    [[nodiscard]] QString question() const;
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] AnswerResult result() const;
    [[nodiscard]] TraceAnswerDetails details() const;

    void inspect(const QString &traceId, bool isAnswerTrace);
    void forgetSaved();
    void revalidateOnActivation(Qt::ApplicationState state);
    Q_INVOKABLE void reload();

  signals:
    void changed();

  private:
    [[nodiscard]] QString sessionQuestion() const;
    void clearSaved();
    void synchronize();
    void fetch();
    void receive(const openapi::HttpRequestWorker &worker,
                 const openapi::AnswerHistoryEntry &entry);
    void finish(const QString &message);
    void checkExpiry();

    BackendSupervisor &backend_;
    Answers &answers_;
    HistoryPreferences &preferences_;
    LocalApiRequest request_;
    QTimer expiryTimer_;
    SavedAnswer saved_;
    QString traceId_;
    QString feedback_;
    qint64 expiresAt_ = 0;
    bool knownAnswer_ = false;
    bool requested_ = false;
    bool loading_ = false;
};

} // namespace ctxfab
