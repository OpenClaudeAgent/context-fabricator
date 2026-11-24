#include "SavedAnswer.h"

#include <QDateTime>
#include <QLocale>

namespace ctxfab {
namespace {

QString date(qint64 msecs) {
    return QLocale().toString(QDateTime::fromMSecsSinceEpoch(msecs).date(), QLocale::ShortFormat);
}

} // namespace

QString HistorySummary::completedAt() const { return date(entry_.getCompletedAtMs()); }

QString SavedAnswer::completedAt() const { return date(entry_.getCompletedAtMs()); }

qint64 SavedAnswer::expiresAtMs(int retentionDays) const {
    constexpr qint64 MillisecondsPerDay = 86'400'000;
    if (entry_.is_expires_at_ms_Set()) {
        return entry_.getExpiresAtMs();
    }
    // Older servers omit the deadline. Bound the displayed detail by the retention policy.
    return qMin(entry_.getCompletedAtMs(), QDateTime::currentMSecsSinceEpoch()) +
           retentionDays * MillisecondsPerDay + 1;
}

QStringList SavedAnswer::scope() const {
    const auto scope = entry_.getScope();
    QStringList fields;
    if (scope.is_repository_id_Set()) {
        fields.append(tr("Repository %1").arg(QString::number(scope.getRepositoryId())));
    }
    if (scope.is_branch_Set()) {
        fields.append(tr("Branch: %1").arg(scope.getBranch()));
    }
    if (scope.is_history_commit_Set()) {
        fields.append(tr("Commit: %1").arg(scope.getHistoryCommit()));
    }
    if (scope.is_actions_run_id_Set()) {
        fields.append(tr("Actions run: %1").arg(QString::number(scope.getActionsRunId())));
    }
    if (scope.is_actions_run_attempt_Set()) {
        fields.append(tr("Attempt: %1").arg(QString::number(scope.getActionsRunAttempt())));
    }
    return fields;
}

} // namespace ctxfab
