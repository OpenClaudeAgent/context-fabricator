#pragma once

#include "Diagnostic.h"

#include <QDateTime>
#include <QLocale>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class DiagnosticEvent {
    Q_GADGET
    QML_VALUE_TYPE(diagnosticEvent)
    Q_PROPERTY(QString title READ title CONSTANT FINAL)
    Q_PROPERTY(QString code READ code CONSTANT FINAL)
    Q_PROPERTY(QString observedAt READ observedAt CONSTANT FINAL)

  public:
    DiagnosticEvent() = default;
    explicit DiagnosticEvent(const openapi::Diagnostic &event) : event_(event) {}

    [[nodiscard]] QString title() const { return event_.getMessage(); }
    [[nodiscard]] QString code() const { return event_.getCode(); }
    [[nodiscard]] QString observedAt() const {
        const auto time = QDateTime::fromSecsSinceEpoch(event_.getTimestamp());
        return QLocale().toString(time.date(), QLocale::ShortFormat) + ' ' +
               time.time().toString(QStringLiteral("HH:mm:ss"));
    }

  private:
    openapi::Diagnostic event_;
};

} // namespace ctxfab
