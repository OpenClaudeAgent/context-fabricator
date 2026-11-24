#pragma once

#include "LocalAnswer.h"

#include <QCoreApplication>
#include <QUrl>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class AnswerCitation {
    Q_GADGET
    QML_VALUE_TYPE(answerCitation)
    Q_DECLARE_TR_FUNCTIONS(AnswerCitation)

    Q_PROPERTY(QString label READ label CONSTANT FINAL)
    Q_PROPERTY(QString quote READ quote CONSTANT FINAL)
    Q_PROPERTY(QString comparison READ comparison CONSTANT FINAL)
    Q_PROPERTY(QString workflow READ workflow CONSTANT FINAL)
    Q_PROPERTY(QUrl lines READ lines CONSTANT FINAL)
    Q_PROPERTY(QUrl run READ run CONSTANT FINAL)

  public:
    AnswerCitation() = default;
    explicit AnswerCitation(const openapi::Citation &citation) : citation_(citation) {}

    [[nodiscard]] QString label() const;
    [[nodiscard]] QString quote() const { return citation_.getQuote(); }
    [[nodiscard]] QString comparison() const;
    [[nodiscard]] QString workflow() const;
    [[nodiscard]] QUrl lines() const;
    [[nodiscard]] QUrl run() const;

  private:
    openapi::Citation citation_;
};

class AnswerResult {
    Q_GADGET
    QML_VALUE_TYPE(answerResult)
    Q_DECLARE_TR_FUNCTIONS(AnswerResult)

    Q_PROPERTY(bool present READ present CONSTANT FINAL)
    Q_PROPERTY(bool abstained READ abstained CONSTANT FINAL)
    Q_PROPERTY(QString text READ text CONSTANT FINAL)
    Q_PROPERTY(QList<ctxfab::AnswerCitation> citations READ citations CONSTANT FINAL)

  public:
    AnswerResult() = default;
    explicit AnswerResult(const openapi::LocalAnswer &answer) : answer_(answer) {}

    [[nodiscard]] bool present() const { return answer_.isSet(); }
    [[nodiscard]] bool abstained() const;
    [[nodiscard]] bool affectedBy(const QSet<qint64> &repositories) const;
    [[nodiscard]] QString text() const;
    [[nodiscard]] QList<AnswerCitation> citations() const;
    [[nodiscard]] const openapi::LocalAnswer &payload() const { return answer_; }

  private:
    openapi::LocalAnswer answer_;
};

} // namespace ctxfab
