#include "AnswerResult.h"

#include <QLocale>

#include <algorithm>
#include <array>

namespace ctxfab {
namespace {

QUrl githubUrl(const QString &value) {
    const QUrl url(value, QUrl::StrictMode);
    return url.isValid() && url.scheme() == "https" && url.host() == "github.com" &&
                   url.userInfo().isEmpty() && url.port() == -1
               ? url
               : QUrl{};
}

constexpr std::array explanations{
    std::pair{
        "noCandidates",
        QT_TRANSLATE_NOOP("AnswerResult",
                          "The search returned zero candidates. Answer generation was skipped.")},
    std::pair{
        "noUsableCandidates",
        QT_TRANSLATE_NOOP(
            "AnswerResult",
            "The search returned candidates, but none were available for answer generation.")},
    std::pair{"contextLimit",
              QT_TRANSLATE_NOOP("AnswerResult",
                                "None of the proposed sources fit in the model’s input budget.")},
    std::pair{
        "generationDeadline",
        QT_TRANSLATE_NOOP("AnswerResult",
                          "The model reached its time limit before returning a usable draft.")},
    std::pair{
        "workerUnavailable",
        QT_TRANSLATE_NOOP("AnswerResult",
                          "The local model was unavailable before returning a usable draft.")},
    std::pair{
        "invalidModelOutput",
        QT_TRANSLATE_NOOP("AnswerResult", "The model’s output did not match the response format.")},
    std::pair{
        "noCitations",
        QT_TRANSLATE_NOOP("AnswerResult",
                          "The model returned a draft without citations. The draft was withheld.")},
    std::pair{"citationsUnverified",
              QT_TRANSLATE_NOOP(
                  "AnswerResult",
                  "The draft’s citations did not pass verification against the indexed sources.")},
    std::pair{
        "evidenceChanged",
        QT_TRANSLATE_NOOP("AnswerResult",
                          "The source evidence changed before its citations could be verified.")},
};

} // namespace

QString AnswerCitation::label() const {
    const auto evidence = citation_.getEvidence();
    const auto url = lines();
    if (evidence.is_location_Set() && !url.isEmpty()) {
        const auto fragment = url.hasFragment() ? QStringLiteral("#") + url.fragment() : QString{};
        return QStringLiteral("%1%2 @ %3")
            .arg(evidence.getLocation().getPath(), fragment, evidence.getRevision().left(10));
    }
    return QStringLiteral("%1 @ %2").arg(evidence.getSource(), evidence.getRevision());
}

QString AnswerCitation::comparison() const {
    if (!citation_.is_comparison_Set()) {
        return {};
    }
    const auto comparison = citation_.getComparison();
    const auto side = comparison.getSide() == "before" ? tr("Before") : tr("After");
    return QStringLiteral("%1 · %2 vs %3")
        .arg(side, comparison.getChild().left(10), comparison.getParent().left(10));
}

QString AnswerCitation::workflow() const {
    if (!citation_.is_workflow_run_Set()) {
        return {};
    }
    const auto run = citation_.getWorkflowRun();
    return tr("Run %1 · attempt %2 · reported head %3")
        .arg(QString::number(run.getRunId()), QString::number(run.getAttempt()),
             run.getReportedHead().left(10));
}

QUrl AnswerCitation::lines() const { return githubUrl(citation_.getLineUrl()); }

QUrl AnswerCitation::run() const { return githubUrl(citation_.getWorkflowRun().getRunUrl()); }

bool AnswerResult::abstained() const { return answer_.getOutcome() == "insufficientEvidence"; }

bool AnswerResult::affectedBy(const QSet<qint64> &repositories) const {
    return !repositories.isEmpty() &&
           (answer_.is_evidence_review_Set() ||
            (answer_.getOutcome() == "answered" &&
             (!answer_.is_evidence_repository_ids_Set() ||
              answer_.getEvidenceRepositoryIds().intersects(repositories))));
}

QString AnswerResult::text() const {
    if (!abstained()) {
        return answer_.getText();
    }
    const auto reason = answer_.getEvidenceReview().getReason();
    const auto explanation = std::ranges::find_if(explanations, [&reason](const auto &entry) {
        return reason == QLatin1StringView(entry.first);
    });
    return explanation == explanations.end() ? tr("The local corpus has insufficient evidence.")
                                             : tr(explanation->second);
}

QList<AnswerCitation> AnswerResult::citations() const {
    QList<AnswerCitation> citations;
    const auto evidence = answer_.getCitations();
    citations.reserve(evidence.size());
    for (const auto &citation : evidence) {
        citations.append(AnswerCitation(citation));
    }
    return citations;
}

} // namespace ctxfab
