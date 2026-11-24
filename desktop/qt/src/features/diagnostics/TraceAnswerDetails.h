#pragma once

#include "LocalAnswer.h"

#include <QCoreApplication>
#include <QStringList>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

struct TraceAnswerExcerpt {
    Q_GADGET
    QML_VALUE_TYPE(traceAnswerExcerpt)

    Q_PROPERTY(QString title MEMBER title CONSTANT FINAL)
    Q_PROPERTY(QString location MEMBER location CONSTANT FINAL)
    Q_PROPERTY(QString text READ text CONSTANT FINAL)
    Q_PROPERTY(QString status MEMBER status CONSTANT FINAL)

  public:
    [[nodiscard]] QString text() const { return text_; }
    QString title;
    QString location;
    QString text_;
    QString status;
};

class TraceModelSource {
    Q_GADGET
    QML_VALUE_TYPE(traceModelSource)
    Q_DECLARE_TR_FUNCTIONS(TraceModelSource)

    Q_PROPERTY(QString title READ title CONSTANT FINAL)
    Q_PROPERTY(QString scope READ scope CONSTANT FINAL)
    Q_PROPERTY(QStringList changes READ changes CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::TraceAnswerExcerpt> parts READ parts CONSTANT FINAL)

  public:
    TraceModelSource() = default;
    explicit TraceModelSource(const openapi::AnswerModelSource &source) : source_(source) {}

    [[nodiscard]] QString title() const;
    [[nodiscard]] QString scope() const;
    [[nodiscard]] QStringList changes() const;
    [[nodiscard]] QList<TraceAnswerExcerpt> parts() const;

  private:
    openapi::AnswerModelSource source_;
};

class TraceAnswerDetails {
    Q_GADGET
    QML_VALUE_TYPE(traceAnswerDetails)
    Q_DECLARE_TR_FUNCTIONS(TraceAnswerDetails)

    Q_PROPERTY(bool hasReview READ hasReview CONSTANT FINAL)
    Q_PROPERTY(QString reviewScope READ reviewScope CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::TraceAnswerExcerpt> searches READ searches CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::TraceAnswerExcerpt> candidates READ candidates CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::TraceModelSource> sources READ sources CONSTANT FINAL)

  public:
    TraceAnswerDetails() = default;
    explicit TraceAnswerDetails(const openapi::LocalAnswer &answer) : answer_(answer) {}

    [[nodiscard]] bool hasReview() const { return answer_.is_evidence_review_Set(); }
    [[nodiscard]] QString question() const;
    [[nodiscard]] QString reviewScope() const;
    [[nodiscard]] QList<TraceAnswerExcerpt> searches() const;
    [[nodiscard]] QList<TraceAnswerExcerpt> candidates() const;
    [[nodiscard]] QList<TraceModelSource> sources() const;

  private:
    openapi::LocalAnswer answer_;
};

} // namespace ctxfab
