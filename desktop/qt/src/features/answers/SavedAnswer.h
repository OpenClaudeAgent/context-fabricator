#pragma once

#include "AnswerHistoryEntry.h"
#include "AnswerHistorySummary.h"
#include "AnswerResult.h"

#include <QCoreApplication>
#include <QStringList>

namespace ctxfab {

class HistorySummary {
    Q_GADGET
    QML_VALUE_TYPE(historySummary)

    Q_PROPERTY(QString key READ key CONSTANT FINAL)
    Q_PROPERTY(QString question READ question CONSTANT FINAL)
    Q_PROPERTY(QString completedAt READ completedAt CONSTANT FINAL)

  public:
    HistorySummary() = default;
    explicit HistorySummary(const openapi::AnswerHistorySummary &entry) : entry_(entry) {}

    [[nodiscard]] QString key() const { return QString::number(entry_.getId()); }
    [[nodiscard]] QString question() const { return entry_.getQuestion(); }
    [[nodiscard]] QString completedAt() const;

  private:
    openapi::AnswerHistorySummary entry_;
};

class SavedAnswer {
    Q_GADGET
    QML_VALUE_TYPE(savedAnswer)
    Q_DECLARE_TR_FUNCTIONS(SavedAnswer)

    Q_PROPERTY(bool present READ present CONSTANT FINAL)
    Q_PROPERTY(QString question READ question CONSTANT FINAL)
    Q_PROPERTY(QString traceId READ traceId CONSTANT FINAL)
    Q_PROPERTY(QString completedAt READ completedAt CONSTANT FINAL)
    Q_PROPERTY(QStringList scope READ scope CONSTANT FINAL)
    Q_PROPERTY(ctxfab::AnswerResult result READ result CONSTANT FINAL)

  public:
    SavedAnswer() = default;
    explicit SavedAnswer(const openapi::AnswerHistoryEntry &entry) : entry_(entry) {}

    [[nodiscard]] bool present() const { return entry_.is_id_Set(); }
    [[nodiscard]] QString question() const { return entry_.getQuestion(); }
    [[nodiscard]] QString traceId() const { return entry_.getTraceId(); }
    [[nodiscard]] QString completedAt() const;
    [[nodiscard]] QStringList scope() const;
    [[nodiscard]] qint64 expiresAtMs(int retentionDays) const;
    [[nodiscard]] AnswerResult result() const { return AnswerResult(entry_.getAnswer()); }

  private:
    openapi::AnswerHistoryEntry entry_;
};

} // namespace ctxfab
