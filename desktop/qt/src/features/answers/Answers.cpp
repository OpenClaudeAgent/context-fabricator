#include "Answers.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QRegularExpression>
#include <QUuid>

#include <algorithm>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
constexpr int SessionQuestionLimit = 20;

QString answerTraceId(const Worker &worker) {
    static const QRegularExpression pattern(QStringLiteral("\\A[0-9a-f]{32}\\z"));
    const auto headers = worker.getResponseHeaders();
    for (auto it = headers.cbegin(); it != headers.cend(); ++it) {
        if (it.key().compare(QLatin1StringView("X-Context-Trace-Id"), Qt::CaseInsensitive) == 0 &&
            pattern.match(it.value()).hasMatch()) {
            return it.value();
        }
    }
    return {};
}

bool delivered(const openapi::LocalAnswer &answer) {
    if (!answer.isValid()) {
        return false;
    }
    if (answer.getOutcome() == "insufficientEvidence") {
        return answer.getCitations().isEmpty();
    }
    return answer.getOutcome() == "answered" && !answer.getText().trimmed().isEmpty() &&
           !answer.getCitations().isEmpty();
}

} // namespace

QString SessionQuestion::outcome() const {
    switch (outcome_) {
    case Outcome::Answered:
        return tr("Answered");
    case Outcome::Abstained:
        return tr("Insufficient evidence");
    case Outcome::Ended:
        return tr("Request ended");
    }
    return {};
}

Answers::Answers(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &Answers::backendChanged);
}

bool Answers::ready() const { return backend_.session().has_value(); }

bool Answers::canAsk() const {
    return ready() && !busy() && !modelsBusy() && activeModelIndex() >= 0 &&
           !question_.trimmed().isEmpty() && !questionTooLong();
}

bool Answers::questionTooLong() const { return question_.trimmed().toUcs4().size() > 512; }

QStringList Answers::modelLabels() const {
    QStringList labels;
    const auto profiles = models_.getProfiles();
    labels.reserve(profiles.size());
    for (const auto &profile : profiles) {
        labels.append(profile.getLabel());
    }
    return labels;
}

int Answers::activeModelIndex() const {
    const auto active = activeModel();
    const auto profiles = models_.getProfiles();
    const auto found = std::ranges::find_if(
        profiles, [&active](const auto &profile) { return profile.getId() == active; });
    return found == profiles.end() ? -1 : static_cast<int>(found - profiles.begin());
}

QString Answers::questionForTrace(const QString &traceId) const {
    const auto found = std::ranges::find(sessionQuestions_, traceId, &SessionQuestion::traceId);
    return found == sessionQuestions_.end() ? QString{} : found->text();
}

void Answers::setQuestion(const QString &question) {
    if (question != question_) {
        question_ = question;
        emit changed();
    }
}

void Answers::backendChanged() {
    answerRequest_.cancel();
    cancellationRequest_.cancel();
    modelsRequest_.cancel();
    requestId_.clear();
    submittedQuestion_.clear();
    traceId_.clear();
    feedback_.clear();
    modelFeedback_.clear();
    purgedRepositories_.clear();
    rememberRequest_ = false;
    result_ = {};
    models_ = {};
    cancelling_ = false;
    modelsBusy_ = false;
    emit changed();
    refreshModels();
}

void Answers::refreshModels() {
    if (!modelsBusy_) {
        loadModels({});
    }
}

void Answers::selectModel(int index) {
    const auto profiles = models_.getProfiles();
    if (!busy() && !modelsBusy_ && index >= 0 && index < profiles.size() &&
        index != activeModelIndex()) {
        loadModels(profiles.at(index).getId());
    }
}

void Answers::loadModels(const QString &profile) {
    const auto session = backend_.session();
    if (!session) {
        return;
    }
    const auto timeout =
        profile.isEmpty() ? std::nullopt : std::optional{InferenceRequestTimeoutMs};
    auto *api = modelsRequest_.begin(*session, timeout);
    const auto received = [this](const Worker *worker, const openapi::GenerationModels &models) {
        if (worker->getHttpResponseCode() != 200 || !models.isValid()) {
            finishModels(tr("Invalid response from the backend"));
            return;
        }
        models_ = models;
        finishModels({});
    };
    const auto failed = [this](const Worker *worker) {
        finishModels(LocalApiRequest::rejection(*worker, tr("Backend request failed")));
    };
    modelsBusy_ = true;
    modelFeedback_.clear();
    if (profile.isEmpty()) {
        connect(api, &Api::readGenerationModelsSignalFull, this, received);
        connect(api, &Api::readGenerationModelsSignalErrorFull, this, failed);
        api->readGenerationModels();
    } else {
        connect(api, &Api::selectGenerationModelSignalFull, this, received);
        connect(api, &Api::selectGenerationModelSignalErrorFull, this, failed);
        openapi::GenerationSelection selection;
        selection.setProfile(profile);
        api->selectGenerationModel(selection);
    }
    emit changed();
}

void Answers::finishModels(const QString &message) {
    modelsRequest_.cancel();
    modelFeedback_ = message;
    modelsBusy_ = false;
    emit changed();
}

void Answers::ask() {
    const auto session = backend_.session();
    if (!session || !canAsk()) {
        return;
    }
    submittedQuestion_ = question_.trimmed();
    requestId_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
    rememberRequest_ = true;
    result_ = {};
    traceId_.clear();
    feedback_.clear();
    openapi::Question question;
    question.setQuery(submittedQuestion_);
    question.setRequestId(requestId_);
    question.setIncludeModelContext(true);

    auto *api = answerRequest_.begin(*session);
    connect(api, &Api::askQuestionSignalFull, this,
            [this](const Worker *worker, const openapi::LocalAnswer &answer) {
                acceptAnswer(*worker, answer);
            });
    connect(api, &Api::askQuestionSignalErrorFull, this,
            [this](const Worker *worker) { rejectAnswer(*worker); });
    api->askQuestion(question);
    emit changed();
}

void Answers::acceptAnswer(const Worker &worker, const openapi::LocalAnswer &answer) {
    traceId_ = answerTraceId(worker);
    if (worker.getHttpResponseCode() != 200 || !delivered(answer)) {
        finishAnswer(tr("Invalid response from the backend"));
        return;
    }
    const AnswerResult result(answer);
    if (result.affectedBy(purgedRepositories_)) {
        traceId_.clear();
        finishAnswer(tr("Local repository data used by this answer was deleted."));
        return;
    }
    result_ = result;
    finishAnswer({});
}

void Answers::rejectAnswer(const Worker &worker) {
    traceId_ = answerTraceId(worker);
    const openapi::Problem problem(QString::fromUtf8(worker.response));
    const bool cancelled =
        worker.getHttpResponseCode() == 409 && problem.getCode() == "answer_cancelled";
    finishAnswer(cancelled ? tr("Answer request cancelled.")
                           : LocalApiRequest::rejection(worker, tr("Backend request failed")));
}

void Answers::finishAnswer(const QString &message) {
    answerRequest_.cancel();
    cancellationRequest_.cancel();
    requestId_.clear();
    purgedRepositories_.clear();
    cancelling_ = false;
    feedback_ = message;
    rememberQuestion();
    emit changed();
    emit requestFinished();
}

void Answers::rememberQuestion() {
    if (!rememberRequest_ || traceId_.isEmpty()) {
        return;
    }
    using Outcome = SessionQuestion::Outcome;
    auto outcome = Outcome::Ended;
    if (result_.present()) {
        outcome = result_.abstained() ? Outcome::Abstained : Outcome::Answered;
    }
    sessionQuestions_.prepend(SessionQuestion(traceId_, submittedQuestion_, outcome));
    if (sessionQuestions_.size() > SessionQuestionLimit) {
        sessionQuestions_.removeLast();
    }
    rememberRequest_ = false;
    emit sessionQuestionsChanged();
}

void Answers::cancelAnswer() {
    const auto session = backend_.session();
    if (!session || !busy() || cancelling_) {
        return;
    }
    auto *api = cancellationRequest_.begin(*session);
    connect(api, &Api::cancelAnswerSignalFull, this,
            [this](const Worker *worker, const openapi::AnswerCancellation &response) {
                cancellationRequest_.cancel();
                if (worker->getHttpResponseCode() != 200 || !response.isValid() ||
                    response.getRequestId() != requestId_ || response.getStatus() != "cancelled") {
                    cancelFailed(tr("Invalid response from the backend"));
                }
            });
    connect(api, &Api::cancelAnswerSignalErrorFull, this, [this](const Worker *worker) {
        cancelFailed(LocalApiRequest::rejection(*worker, tr("Backend request failed")));
    });
    cancelling_ = true;
    feedback_.clear();
    api->cancelAnswer(requestId_);
    emit changed();
}

void Answers::cancelFailed(const QString &message) {
    cancellationRequest_.cancel();
    cancelling_ = false;
    feedback_ = message;
    emit changed();
}

void Answers::repositoryPurged(qint64 repository) {
    submittedQuestion_.clear();
    if (busy()) {
        purgedRepositories_.insert(repository);
    }
    if (result_.affectedBy({repository})) {
        result_ = {};
        traceId_.clear();
        feedback_ = tr("Local repository data used by this answer was deleted.");
    }
    discardSessionQuestions();
}

void Answers::discardSessionQuestions() {
    rememberRequest_ = false;
    if (!sessionQuestions_.isEmpty()) {
        sessionQuestions_.clear();
        emit sessionQuestionsChanged();
    }
    emit changed();
}

} // namespace ctxfab
