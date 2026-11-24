#pragma once

#include "AnswerResult.h"
#include "GenerationModels.h"
#include "LocalApiRequest.h"

#include <QObject>
#include <QStringList>
#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

class BackendSupervisor;

class SessionQuestion {
    Q_GADGET
    QML_VALUE_TYPE(sessionQuestion)
    Q_DECLARE_TR_FUNCTIONS(SessionQuestion)

    Q_PROPERTY(QString traceId READ traceId CONSTANT FINAL)
    Q_PROPERTY(QString text READ text CONSTANT FINAL)
    Q_PROPERTY(QString outcome READ outcome CONSTANT FINAL)

  public:
    enum class Outcome { Answered, Abstained, Ended };

    SessionQuestion() = default;
    SessionQuestion(const QString &trace, const QString &text, Outcome outcome)
        : trace_(trace), text_(text), outcome_(outcome) {}

    [[nodiscard]] QString traceId() const { return trace_; }
    [[nodiscard]] QString text() const { return text_; }
    [[nodiscard]] QString outcome() const;

  private:
    QString trace_;
    QString text_;
    Outcome outcome_ = Outcome::Ended;
};

class Answers final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")

    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool cancelling READ cancelling NOTIFY changed FINAL)
    Q_PROPERTY(bool canAsk READ canAsk NOTIFY changed FINAL)
    Q_PROPERTY(bool questionTooLong READ questionTooLong NOTIFY changed FINAL)
    Q_PROPERTY(bool modelsBusy READ modelsBusy NOTIFY changed FINAL)
    Q_PROPERTY(QStringList modelLabels READ modelLabels NOTIFY changed FINAL)
    Q_PROPERTY(int activeModelIndex READ activeModelIndex NOTIFY changed FINAL)
    Q_PROPERTY(QString modelFeedback READ modelFeedback NOTIFY changed FINAL)
    Q_PROPERTY(QString question READ question NOTIFY changed FINAL)
    Q_PROPERTY(QString submittedQuestion READ submittedQuestion NOTIFY changed FINAL)
    Q_PROPERTY(QString traceId READ traceId NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::AnswerResult result READ result NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::SessionQuestion> sessionQuestions READ sessionQuestions NOTIFY
                   sessionQuestionsChanged FINAL)

  public:
    explicit Answers(BackendSupervisor &backend);

    [[nodiscard]] bool ready() const;
    [[nodiscard]] bool busy() const { return !requestId_.isEmpty(); }
    [[nodiscard]] bool cancelling() const { return cancelling_; }
    [[nodiscard]] bool canAsk() const;
    [[nodiscard]] bool questionTooLong() const;
    [[nodiscard]] bool modelsBusy() const { return modelsBusy_; }
    [[nodiscard]] QStringList modelLabels() const;
    [[nodiscard]] QString activeModel() const { return models_.getActiveProfile(); }
    [[nodiscard]] int activeModelIndex() const;
    [[nodiscard]] QString modelFeedback() const { return modelFeedback_; }
    [[nodiscard]] QString question() const { return question_; }
    [[nodiscard]] QString submittedQuestion() const { return submittedQuestion_; }
    [[nodiscard]] QString traceId() const { return traceId_; }
    [[nodiscard]] QString feedback() const { return feedback_; }
    [[nodiscard]] AnswerResult result() const { return result_; }
    [[nodiscard]] QList<SessionQuestion> sessionQuestions() const { return sessionQuestions_; }
    [[nodiscard]] QString questionForTrace(const QString &traceId) const;

    Q_INVOKABLE void setQuestion(const QString &question);
    Q_INVOKABLE void refreshModels();
    Q_INVOKABLE void selectModel(int index);
    Q_INVOKABLE void ask();
    Q_INVOKABLE void cancelAnswer();
    void repositoryPurged(qint64 repository);
    void discardSessionQuestions();

  signals:
    void changed();
    void sessionQuestionsChanged();
    void requestFinished();

  private:
    static constexpr int InferenceRequestTimeoutMs = 90'000;

    void backendChanged();
    void loadModels(const QString &profile);
    void finishModels(const QString &message);
    void acceptAnswer(const openapi::HttpRequestWorker &worker, const openapi::LocalAnswer &answer);
    void rejectAnswer(const openapi::HttpRequestWorker &worker);
    void finishAnswer(const QString &message);
    void rememberQuestion();
    void cancelFailed(const QString &message);

    BackendSupervisor &backend_;
    LocalApiRequest answerRequest_{InferenceRequestTimeoutMs};
    LocalApiRequest cancellationRequest_;
    LocalApiRequest modelsRequest_;
    openapi::GenerationModels models_;
    AnswerResult result_;
    QList<SessionQuestion> sessionQuestions_;
    QSet<qint64> purgedRepositories_;
    QString requestId_;
    QString question_;
    QString submittedQuestion_;
    QString traceId_;
    QString feedback_;
    QString modelFeedback_;
    bool cancelling_ = false;
    bool modelsBusy_ = false;
    bool rememberRequest_ = false;
};

} // namespace ctxfab
