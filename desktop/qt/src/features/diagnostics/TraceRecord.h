#pragma once

#include "RecordedAnswerDetails.h"
#include "SignalSample.h"

#include <QCoreApplication>
#include <QStringList>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class TraceRecord {
    Q_GADGET
    QML_VALUE_TYPE(traceRecord)
    Q_DECLARE_TR_FUNCTIONS(TraceRecord)

    Q_PROPERTY(QString name READ name CONSTANT FINAL)
    Q_PROPERTY(QString title READ title CONSTANT FINAL)
    Q_PROPERTY(QString outcome READ outcome CONSTANT FINAL)
    Q_PROPERTY(QString level READ level CONSTANT FINAL)
    Q_PROPERTY(bool debugLevel READ isDebug CONSTANT FINAL)
    Q_PROPERTY(bool errorLevel READ isError CONSTANT FINAL)
    Q_PROPERTY(QString duration READ duration CONSTANT FINAL)
    Q_PROPERTY(QString count READ count CONSTANT FINAL)
    Q_PROPERTY(bool failed READ failed CONSTANT FINAL)
    Q_PROPERTY(bool succeeded READ succeeded CONSTANT FINAL)
    Q_PROPERTY(QString operation READ operation CONSTANT FINAL)
    Q_PROPERTY(QString httpRequest READ httpRequest CONSTANT FINAL)
    Q_PROPERTY(QString httpStatus READ httpStatus CONSTANT FINAL)
    Q_PROPERTY(QString problemCode READ problemCode CONSTANT FINAL)
    Q_PROPERTY(QString client READ client CONSTANT FINAL)
    Q_PROPERTY(QString modelProfile READ modelProfile CONSTANT FINAL)
    Q_PROPERTY(ctxfab::RecordedAnswerDetails answerDetails READ answerDetails CONSTANT FINAL)
    Q_PROPERTY(QString observedAt READ observedAt CONSTANT FINAL)
    Q_PROPERTY(QString timestamp READ timestamp CONSTANT FINAL)
    Q_PROPERTY(QString traceId READ traceId CONSTANT FINAL)
    Q_PROPERTY(QString spanId READ spanId CONSTANT FINAL)
    Q_PROPERTY(QString parentSpanId READ parentSpanId CONSTANT FINAL)
    Q_PROPERTY(QString workTraceId READ workTraceId CONSTANT FINAL)
    Q_PROPERTY(QStringList events READ events CONSTANT FINAL)
    Q_PROPERTY(int depth READ depth CONSTANT FINAL)
    Q_PROPERTY(bool parentMissing READ parentMissing CONSTANT FINAL)
    Q_PROPERTY(bool hasStart READ hasStart CONSTANT FINAL)

  public:
    TraceRecord() = default;
    explicit TraceRecord(const openapi::SignalSample &sample, int depth = 0,
                         bool parentMissing = false);

    [[nodiscard]] static QString formatDuration(qint64 micros);
    [[nodiscard]] static bool isValidTraceId(const QString &value);
    [[nodiscard]] static QString clientName(const openapi::ClientKind &client);

    [[nodiscard]] qint64 id() const { return sample_.getId(); }
    [[nodiscard]] QString name() const { return sample_.getName(); }
    [[nodiscard]] QString title() const;
    [[nodiscard]] QString outcome() const;
    [[nodiscard]] QString level() const;
    [[nodiscard]] bool isDebug() const { return sample_.getLevel() == "debug"; }
    [[nodiscard]] bool isError() const { return sample_.getLevel() == "error"; }
    [[nodiscard]] QString duration() const;
    [[nodiscard]] QString count() const;
    [[nodiscard]] bool failed() const;
    [[nodiscard]] bool succeeded() const;
    [[nodiscard]] QString operation() const { return sample_.getOperation(); }
    [[nodiscard]] QString httpRequest() const;
    [[nodiscard]] QString httpStatus() const;
    [[nodiscard]] QString problemCode() const { return sample_.getProblemCode(); }
    [[nodiscard]] QString client() const;
    [[nodiscard]] QString modelProfile() const { return sample_.getModelProfile(); }
    [[nodiscard]] RecordedAnswerDetails answerDetails() const {
        return RecordedAnswerDetails(sample_);
    }
    [[nodiscard]] QString observedAt() const;
    [[nodiscard]] QString timestamp() const;
    [[nodiscard]] QString traceId() const { return sample_.getTraceId(); }
    [[nodiscard]] QString spanId() const { return sample_.getSpanId(); }
    [[nodiscard]] QString parentSpanId() const { return sample_.getParentSpanId(); }
    [[nodiscard]] QString workTraceId() const { return sample_.getWorkTraceId(); }
    [[nodiscard]] QStringList events() const;
    [[nodiscard]] int depth() const { return depth_; }
    [[nodiscard]] bool parentMissing() const { return parentMissing_; }
    [[nodiscard]] bool hasStart() const { return sample_.is_started_at_unix_micros_Set(); }

  private:
    openapi::SignalSample sample_;
    int depth_ = 0;
    bool parentMissing_ = false;
};

} // namespace ctxfab
