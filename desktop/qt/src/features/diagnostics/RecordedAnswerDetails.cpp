#include "RecordedAnswerDetails.h"

#include "TraceRecord.h"

#include <QLocale>

#include <algorithm>
#include <array>

namespace ctxfab {
namespace {

constexpr std::array BudgetKinds{
    std::pair{"passages", QT_TRANSLATE_NOOP("RecordedAnswerDetails", "%1 of %2 passages selected")},
    std::pair{"linkedPassages",
              QT_TRANSLATE_NOOP("RecordedAnswerDetails", "%1 of %2 linked passages selected")},
    std::pair{"historyPairs",
              QT_TRANSLATE_NOOP("RecordedAnswerDetails", "%1 of %2 history pairs selected")},
    std::pair{"workflowAttempt",
              QT_TRANSLATE_NOOP("RecordedAnswerDetails", "%1 of %2 workflow attempts selected")},
};

} // namespace

QString RecordedAnswerDetails::budgetSummary() const {
    if (!sample_.is_context_budget_Set()) {
        return {};
    }
    const auto budget = sample_.getContextBudget();
    const auto kind = budget.getKind();
    const auto found = std::ranges::find_if(
        BudgetKinds, [&kind](const auto &entry) { return kind == QLatin1StringView(entry.first); });
    if (found == BudgetKinds.end()) {
        return {};
    }
    const QLocale locale;
    return tr(found->second)
        .arg(locale.toString(budget.getAdmitted()), locale.toString(budget.getProposed()));
}

QString RecordedAnswerDetails::tokenBudget() const {
    if (!sample_.is_context_budget_Set()) {
        return {};
    }
    const auto budget = sample_.getContextBudget();
    const QLocale locale;
    const auto limit = locale.toString(budget.getInputTokenLimit());
    return budget.is_input_tokens_Set()
               ? tr("%1 of %2 input tokens").arg(locale.toString(budget.getInputTokens()), limit)
               : tr("No evidence fit within the %1-token limit.").arg(limit);
}

QString RecordedAnswerDetails::tokenChecks() const {
    return sample_.is_context_budget_Set()
               ? QLocale().toString(sample_.getContextBudget().getMeasurements())
               : QString{};
}

QList<TraceDetailField> RecordedAnswerDetails::inference() const {
    if (!sample_.is_inference_timing_Set()) {
        return {};
    }
    const auto timing = sample_.getInferenceTiming();
    const QLocale locale;
    return {{tr("Runtime to first token"), TraceRecord::formatDuration(timing.getPromptMicros())},
            {tr("Decode time"), TraceRecord::formatDuration(timing.getGenerationMicros())},
            {tr("Prompt tokens processed"), locale.toString(timing.getProcessedPromptTokens())},
            {tr("Prompt tokens cached"), locale.toString(timing.getCachedPromptTokens())},
            {tr("Tokens generated"), locale.toString(timing.getGeneratedTokens())}};
}

QList<TraceDetailField> RecordedAnswerDetails::retrieval() const {
    if (!sample_.is_retrieval_counts_Set()) {
        return {};
    }
    const auto counts = sample_.getRetrievalCounts();
    const QLocale locale;
    QList<TraceDetailField> rows;
    if (counts.is_passage_candidates_Set()) {
        rows.append({tr("Ranked local passages"), locale.toString(counts.getPassageCandidates())});
    }
    if (counts.is_history_pairs_Set()) {
        rows.append({tr("GitHub history pairs"), locale.toString(counts.getHistoryPairs())});
    }
    if (counts.is_workflow_attempts_Set()) {
        rows.append({tr("GitHub Actions attempts"), locale.toString(counts.getWorkflowAttempts())});
    }
    rows.append({tr("Proposed for generation"), locale.toString(counts.getProposedSources())});
    return rows;
}

QList<TraceDetailField> RecordedAnswerDetails::fusion() const {
    if (!sample_.is_fusion_ranking_Set()) {
        return {};
    }
    const auto ranking = sample_.getFusionRanking();
    const QLocale locale;
    return {{tr("Lexical candidates"), locale.toString(ranking.getLexicalCandidates())},
            {tr("Dense candidates"), locale.toString(ranking.getDenseCandidates())},
            {tr("Rank offset"), locale.toString(ranking.getRankOffset())},
            {tr("Dense weight"), locale.toString(ranking.getDenseWeight(), 'g', 8)}};
}

TraceDetailField RecordedAnswerDetails::describe(const openapi::FusionCandidate &candidate) {
    const QLocale locale;
    QStringList heading{candidate.is_final_rank_Set()
                            ? tr("Fusion #%1").arg(locale.toString(candidate.getFinalRank()))
                            : tr("Dropped at top-20 cutoff"),
                        tr("Score #%1").arg(locale.toString(candidate.getScoreRank()))};
    if (candidate.isLexicalPromoted()) {
        heading.append(tr("Lexical promotion"));
    }
    QStringList summary;
    if (candidate.is_lexical_rank_Set()) {
        summary.append(tr("Lexical #%1").arg(locale.toString(candidate.getLexicalRank())));
    }
    if (candidate.is_dense_rank_Set()) {
        summary.append(tr("Dense #%1").arg(locale.toString(candidate.getDenseRank())));
    }
    summary.append(tr("Score %1").arg(locale.toString(candidate.getScore(), 'g', 8)));
    return {heading.join(QStringLiteral(" · ")), summary.join(QStringLiteral(" · "))};
}

QList<TraceDetailField> RecordedAnswerDetails::fusionCandidates() const {
    QList<TraceDetailField> rows;
    if (sample_.is_fusion_ranking_Set()) {
        for (const auto &candidate : sample_.getFusionRanking().getCandidates()) {
            rows.append(describe(candidate));
        }
    }
    return rows;
}

} // namespace ctxfab
