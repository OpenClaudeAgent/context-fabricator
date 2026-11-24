#pragma once

#include "SignalSample.h"

#include <QCoreApplication>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

struct TraceDetailField {
    Q_GADGET
    QML_VALUE_TYPE(traceDetailField)
    Q_PROPERTY(QString title MEMBER title CONSTANT FINAL)
    Q_PROPERTY(QString value MEMBER value CONSTANT FINAL)

  public:
    QString title;
    QString value;
};

class RecordedAnswerDetails {
    Q_GADGET
    QML_VALUE_TYPE(recordedAnswerDetails)
    Q_DECLARE_TR_FUNCTIONS(RecordedAnswerDetails)

    Q_PROPERTY(QString budgetSummary READ budgetSummary CONSTANT FINAL)
    Q_PROPERTY(QString tokenBudget READ tokenBudget CONSTANT FINAL)
    Q_PROPERTY(QString tokenChecks READ tokenChecks CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::TraceDetailField> inference READ inference CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::TraceDetailField> retrieval READ retrieval CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::TraceDetailField> fusion READ fusion CONSTANT FINAL)
    Q_PROPERTY(
        QList<ctxfab::TraceDetailField> fusionCandidates READ fusionCandidates CONSTANT FINAL)

  public:
    RecordedAnswerDetails() = default;
    explicit RecordedAnswerDetails(const openapi::SignalSample &sample) : sample_(sample) {}

    [[nodiscard]] QString budgetSummary() const;
    [[nodiscard]] QString tokenBudget() const;
    [[nodiscard]] QString tokenChecks() const;
    [[nodiscard]] QList<TraceDetailField> inference() const;
    [[nodiscard]] QList<TraceDetailField> retrieval() const;
    [[nodiscard]] QList<TraceDetailField> fusion() const;
    [[nodiscard]] QList<TraceDetailField> fusionCandidates() const;

  private:
    static TraceDetailField describe(const openapi::FusionCandidate &candidate);
    openapi::SignalSample sample_;
};

} // namespace ctxfab
