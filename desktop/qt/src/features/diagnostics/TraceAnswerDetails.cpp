#include "TraceAnswerDetails.h"

#include <QLocale>

#include <algorithm>
#include <array>

namespace ctxfab {
namespace {

using EvidenceSource = openapi::AnswerEvidenceSource::eAnswerEvidenceSource;

QString evidenceSource(EvidenceSource source) {
    switch (source) {
    case EvidenceSource::LOCALPASSAGES:
        return TraceAnswerDetails::tr("Local passage index");
    case EvidenceSource::GITHUBHISTORY:
        return TraceAnswerDetails::tr("GitHub change history");
    case EvidenceSource::GITHUBACTIONS:
        return TraceAnswerDetails::tr("GitHub Actions attempts");
    case EvidenceSource::INVALID_VALUE_OPENAPI_GENERATED:
    case EvidenceSource::UNKNOWN_DEFAULT_OPEN_API:
        return {};
    }
    return {};
}

constexpr std::array dispositions{
    std::pair{"candidateLimit",
              QT_TRANSLATE_NOOP("TraceAnswerDetails", "Outside the selection limit.")},
    std::pair{
        "passageUnavailable",
        QT_TRANSLATE_NOOP("TraceAnswerDetails",
                          "The indexed passage could not be reopened in the searched scope.")},
    std::pair{
        "fixtureExcluded",
        QT_TRANSLATE_NOOP("TraceAnswerDetails",
                          "The reformulated search excluded test fixtures for this question.")},
    std::pair{"contextLimit",
              QT_TRANSLATE_NOOP("TraceAnswerDetails", "Excluded by the model’s input budget.")},
    std::pair{"noUsableDraft",
              QT_TRANSLATE_NOOP("TraceAnswerDetails",
                                "Proposed as evidence. The model returned no usable draft.")},
    std::pair{"notCited",
              QT_TRANSLATE_NOOP("TraceAnswerDetails",
                                "Sent to the model. The draft did not cite this source.")},
    std::pair{
        "citationUnverified",
        QT_TRANSLATE_NOOP("TraceAnswerDetails",
                          "Cited by the draft. Verification failed for the draft’s citations.")},
    std::pair{"evidenceChanged",
              QT_TRANSLATE_NOOP("TraceAnswerDetails",
                                "The evidence changed before verification finished.")},
};

constexpr std::array searchMethods{
    std::pair{"bm25", QT_TRANSLATE_NOOP("TraceAnswerDetails", "Keyword search (BM25)")},
    std::pair{"semantic", QT_TRANSLATE_NOOP("TraceAnswerDetails", "Semantic search")},
    std::pair{"hybrid", QT_TRANSLATE_NOOP("TraceAnswerDetails", "Keyword and semantic search")},
    std::pair{"selected", QT_TRANSLATE_NOOP("TraceAnswerDetails", "Selected source")},
};

QString translatedValue(const auto &values, const QString &key) {
    const auto found = std::ranges::find_if(
        values, [&key](const auto &value) { return key == QLatin1StringView(value.first); });
    return found == values.end() ? QString{} : TraceAnswerDetails::tr(found->second);
}

QString lineRange(int start, int end) {
    const QLocale locale;
    if (start <= 0) {
        return {};
    }
    return end <= start ? TraceAnswerDetails::tr("Line %1").arg(locale.toString(start))
                        : TraceAnswerDetails::tr("Lines %1–%2")
                              .arg(locale.toString(start), locale.toString(end));
}

QString joined(QStringList fields) {
    fields.removeAll(QString{});
    return fields.join(QStringLiteral(" · "));
}

QString kindName(const QString &kind) {
    if (kind == "current") {
        return TraceModelSource::tr("Current code");
    }
    if (kind == "history") {
        return TraceModelSource::tr("History");
    }
    return TraceModelSource::tr("GitHub Actions");
}

QString partLocation(const openapi::AnswerModelPart &part) {
    const auto side = part.getSide();
    const auto name = side.isEmpty()     ? QString{}
                      : side == "before" ? TraceModelSource::tr("Before")
                                         : TraceModelSource::tr("After");
    const auto revision = part.getRevision();
    return joined({name, part.getPath(),
                   revision.isEmpty() ? QString{} : QStringLiteral("@ ") + revision,
                   lineRange(part.getStartLine(), part.getEndLine())});
}

QString searchDetail(const openapi::AnswerEvidenceSearch &search, const QString &mode) {
    if (search.is_selection_Set()) {
        return search.getSelection();
    }
    return search.getSource().getValue() == EvidenceSource::LOCALPASSAGES
               ? translatedValue(searchMethods, mode)
               : QString{};
}

QString candidateLocation(const openapi::AnswerEvidenceCandidate &candidate) {
    const QLocale locale;
    return joined({
        evidenceSource(candidate.getSource().getValue()),
        candidate.getRevision(),
        lineRange(candidate.getStartLine(), candidate.getEndLine()),
        candidate.is_rank_Set()
            ? TraceAnswerDetails::tr("Search rank %1").arg(locale.toString(candidate.getRank()))
            : QString{},
        candidate.is_slot_Set()
            ? TraceAnswerDetails::tr("Proposed input %1").arg(locale.toString(candidate.getSlot()))
            : QString{},
    });
}

} // namespace

QString TraceModelSource::title() const {
    return QStringLiteral("%1. %2").arg(QLocale().toString(source_.getSlot()),
                                        kindName(source_.getKind()));
}

QString TraceModelSource::scope() const {
    return joined({source_.getRepository(), source_.getBranch()});
}

QStringList TraceModelSource::changes() const {
    if (!source_.is_change_Set()) {
        return {};
    }
    const auto change = source_.getChange();
    return {tr("Before · %1 @ %2").arg(change.getBeforePath(), change.getParent()),
            tr("After · %1 @ %2").arg(change.getAfterPath(), change.getChild())};
}

QList<TraceAnswerExcerpt> TraceModelSource::parts() const {
    QList<TraceAnswerExcerpt> result;
    for (const auto &part : source_.getParts()) {
        result.append({{}, partLocation(part), part.getText(), {}});
    }
    return result;
}

QString TraceAnswerDetails::question() const {
    return hasReview() ? answer_.getEvidenceReview().getQuestion()
                       : answer_.getModelContext().getQuestion();
}

QString TraceAnswerDetails::reviewScope() const {
    const auto review = answer_.getEvidenceReview();
    if (!review.is_scope_Set()) {
        return {};
    }
    const auto scope = review.getScope();
    return tr("Repository %1 · %2")
        .arg(QString::number(scope.getRepositoryId()), scope.getBranch());
}

QList<TraceAnswerExcerpt> TraceAnswerDetails::searches() const {
    QList<TraceAnswerExcerpt> result;
    for (const auto &search : answer_.getEvidenceReview().getSearches()) {
        const auto count = search.getCandidateCount();
        result.append({evidenceSource(search.getSource().getValue()),
                       searchDetail(search, answer_.getMode()),
                       {},
                       count == 1 ? tr("%n candidate", nullptr, count)
                                  : tr("%n candidates", nullptr, count)});
    }
    return result;
}

QList<TraceAnswerExcerpt> TraceAnswerDetails::candidates() const {
    QList<TraceAnswerExcerpt> result;
    for (const auto &candidate : answer_.getEvidenceReview().getCandidates()) {
        result.append({candidate.getReference(), candidateLocation(candidate),
                       candidate.getPreview(),
                       translatedValue(dispositions, candidate.getDisposition())});
    }
    return result;
}

QList<TraceModelSource> TraceAnswerDetails::sources() const {
    QList<TraceModelSource> result;
    for (const auto &source : answer_.getModelContext().getSources()) {
        result.append(TraceModelSource(source));
    }
    return result;
}

} // namespace ctxfab
