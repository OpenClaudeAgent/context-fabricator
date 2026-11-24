#include "GitHubActionsRun.h"

#include <QLocale>
#include <QRegularExpression>

#include <algorithm>
#include <array>

namespace ctxfab {
namespace {

struct StatusLabel {
    QLatin1StringView result;
    const char *label;
};

constexpr std::array Labels{
    StatusLabel{QLatin1StringView("success"), QT_TRANSLATE_NOOP("GitHubActionsRun", "Success")},
    StatusLabel{QLatin1StringView("failure"), QT_TRANSLATE_NOOP("GitHubActionsRun", "Failure")},
    StatusLabel{QLatin1StringView("cancelled"), QT_TRANSLATE_NOOP("GitHubActionsRun", "Cancelled")},
    StatusLabel{QLatin1StringView("queued"), QT_TRANSLATE_NOOP("GitHubActionsRun", "Queued")},
    StatusLabel{QLatin1StringView("in_progress"),
                QT_TRANSLATE_NOOP("GitHubActionsRun", "In Progress")},
    StatusLabel{QLatin1StringView("completed"), QT_TRANSLATE_NOOP("GitHubActionsRun", "Completed")},
    StatusLabel{QLatin1StringView("skipped"), QT_TRANSLATE_NOOP("GitHubActionsRun", "Skipped")},
    StatusLabel{QLatin1StringView("timed_out"), QT_TRANSLATE_NOOP("GitHubActionsRun", "Timed Out")},
};

QUrl attemptUrl(const openapi::GitHubActionRun &run, const QString &repository) {
    static const QRegularExpression repositoryName(
        QStringLiteral("\\A[A-Za-z0-9-]{1,39}/(?!\\.{1,2}\\z)[A-Za-z0-9_.-]{1,100}\\z"));
    if (run.getId() < 1 || run.getAttempt() < 1 || !repositoryName.match(repository).hasMatch()) {
        return {};
    }
    return QUrl(QStringLiteral("https://github.com/%1/actions/runs/%2/attempts/%3")
                    .arg(repository)
                    .arg(run.getId())
                    .arg(run.getAttempt()));
}

} // namespace

GitHubActionsRun::GitHubActionsRun(const openapi::GitHubActionRun &run, const QString &workflow,
                                   const QString &repository)
    : run_(run), workflow_(workflow), url_(attemptUrl(run, repository)) {}

QString GitHubActionsRun::workflow() const {
    return workflow_.isEmpty() ? tr("Workflow #%1").arg(workflowId()) : workflow_;
}

QString GitHubActionsRun::result() const {
    return run_.is_conclusion_Set() ? run_.getConclusion() : run_.getStatus();
}

QString GitHubActionsRun::resultLabel() const {
    auto value = result();
    const auto label = std::ranges::find_if(
        Labels, [&value](const StatusLabel &entry) { return value == entry.result; });
    if (label != Labels.end()) {
        return tr(label->label);
    }
    value.replace('_', ' ');
    if (!value.isEmpty()) {
        value[0] = value.at(0).toUpper();
    }
    return value;
}

QString GitHubActionsRun::summary() const {
    return tr("Run %1 · attempt %2 · %3 · %4")
        .arg(id(), QString::number(attempt()), run_.getEvent(), run_.getHeadSha().left(10));
}

QString GitHubActionsRun::createdAt() const {
    return QLocale().toString(run_.getCreatedAt().toLocalTime(), QLocale::ShortFormat);
}

QString GitHubActionsRun::linkLabel() const {
    return tr("Open run %1, attempt %2 on GitHub").arg(id(), QString::number(attempt()));
}

} // namespace ctxfab
