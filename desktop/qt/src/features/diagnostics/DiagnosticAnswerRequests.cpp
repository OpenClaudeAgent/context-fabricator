#include "DiagnosticAnswerRequests.h"

#include "Answers.h"
#include "TraceAnswerDetails.h"

namespace ctxfab {

DiagnosticAnswerRequests::DiagnosticAnswerRequests(BackendSupervisor &backend, Answers &answers)
    : answers_(answers), feed_(backend, DiagnosticLogs::Feed::AnswerRequests),
      latestTraceId_(answers.traceId()) {
    setSourceModel(&feed_);
    connect(&answers_, &Answers::changed, this, &DiagnosticAnswerRequests::synchronize);
    connect(&answers_, &Answers::sessionQuestionsChanged, this,
            &DiagnosticAnswerRequests::updateQuestions);
    connect(&answers_, &Answers::requestFinished, &feed_, &DiagnosticLogs::refresh);
}

QHash<int, QByteArray> DiagnosticAnswerRequests::roleNames() const {
    auto roles = feed_.roleNames();
    roles.insert(QuestionRole, "question");
    return roles;
}

QVariant DiagnosticAnswerRequests::data(const QModelIndex &index, int role) const {
    if (role != QuestionRole) {
        return QSortFilterProxyModel::data(index, role);
    }
    const auto record = QSortFilterProxyModel::data(index, DiagnosticLogs::SampleRole);
    return answers_.questionForTrace(record.value<TraceRecord>().traceId());
}

bool DiagnosticAnswerRequests::filterAcceptsRow(int sourceRow, const QModelIndex &parent) const {
    if (latestTraceId_.isEmpty()) {
        return true;
    }
    const auto record = feed_.data(feed_.index(sourceRow, 0, parent), DiagnosticLogs::SampleRole);
    return record.value<TraceRecord>().traceId() != latestTraceId_;
}

QString DiagnosticAnswerRequests::latestQuestion() const {
    const auto question = answers_.questionForTrace(latestTraceId_);
    return question.isEmpty() && !latestTraceId_.isEmpty()
               ? TraceAnswerDetails(answers_.result().payload()).question()
               : question;
}

bool DiagnosticAnswerRequests::latestAbstained() const { return answers_.result().abstained(); }

bool DiagnosticAnswerRequests::pending() const { return answers_.busy(); }

void DiagnosticAnswerRequests::synchronize() {
    const auto trace = answers_.traceId();
    if (latestTraceId_ != trace) {
        beginFilterChange();
        latestTraceId_ = trace;
        endFilterChange(Direction::Rows);
        if (trace.isEmpty()) {
            feed_.refresh();
        }
    }
    emit latestChanged();
}

void DiagnosticAnswerRequests::updateQuestions() {
    if (rowCount() > 0) {
        emit dataChanged(index(0, 0), index(rowCount() - 1, 0), {QuestionRole});
    }
}

} // namespace ctxfab
