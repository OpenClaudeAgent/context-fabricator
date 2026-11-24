#include "TraceRecord.h"

#include "SpanEvent.h"

#include <QDateTime>
#include <QLocale>
#include <QRegularExpression>

#include <algorithm>
#include <array>

namespace ctxfab {
namespace {

constexpr qint64 MicrosPerSecond = 1000000;
constexpr std::array DisplayNames{
    QT_TRANSLATE_NOOP("TraceRecord", "GitHub request"),
    QT_TRANSLATE_NOOP("TraceRecord", "Collect GitHub history"),
    QT_TRANSLATE_NOOP("TraceRecord", "Collect GitHub Actions"),
    QT_TRANSLATE_NOOP("TraceRecord", "Refresh GitHub history"),
};

} // namespace

bool TraceRecord::isValidTraceId(const QString &value) {
    static const QRegularExpression pattern(QStringLiteral("\\A[a-f0-9]{32}\\z"));
    return pattern.match(value).hasMatch();
}

QString TraceRecord::clientName(const openapi::ClientKind &client) {
    using Kind = openapi::ClientKind::eClientKind;
    switch (client.getValue()) {
    case Kind::DESKTOP:
        return tr("Desktop");
    case Kind::CLI:
        return tr("CLI");
    default:
        return tr("Other / untagged");
    }
}

QString TraceRecord::client() const {
    return sample_.is_client_kind_Set() ? clientName(sample_.getClientKind()) : QString{};
}

QString TraceRecord::formatDuration(qint64 micros) {
    const QLocale locale;
    if (micros < 1000) {
        return locale.toString(micros) + QStringLiteral(" µs");
    }
    if (micros < MicrosPerSecond) {
        return locale.toString(static_cast<double>(micros) / 1000.0, 'g', 6) +
               QStringLiteral(" ms");
    }
    return locale.toString(static_cast<double>(micros) / 1000000.0, 'g', 9) + QStringLiteral(" s");
}

TraceRecord::TraceRecord(const openapi::SignalSample &sample, int depth, bool parentMissing)
    : sample_(sample), depth_(depth), parentMissing_(parentMissing) {}

QString TraceRecord::title() const {
    if (name() == "api_request" && !sample_.getHttpRoute().isEmpty()) {
        return httpRequest();
    }
    const auto display = sample_.getDisplayName();
    if (!display.isEmpty()) {
        const auto known = std::ranges::find_if(DisplayNames, [&display](const char *name) {
            return display == QLatin1StringView(name);
        });
        return known == DisplayNames.end() ? display : tr(*known);
    }
    auto title = name().replace('_', ' ');
    if (!title.isEmpty()) {
        title[0] = title.at(0).toUpper();
    }
    return title;
}

QString TraceRecord::outcome() const {
    using Outcome = openapi::DiagnosticSpanOutcome::eDiagnosticSpanOutcome;
    switch (sample_.getSpanOutcome().getValue()) {
    case Outcome::SUCCEEDED:
        return tr("Succeeded");
    case Outcome::FAILED:
        return tr("Failed");
    case Outcome::INSUFFICIENTEVIDENCE:
        return tr("Insufficient evidence");
    case Outcome::CANCELLED:
        return tr("Cancelled");
    default:
        return tr("Unknown");
    }
}

QString TraceRecord::level() const {
    const auto value = sample_.getLevel();
    if (value == "info") {
        return tr("Info");
    }
    if (value == "debug") {
        return tr("Debug");
    }
    return value == "error" ? tr("Error") : tr("Unknown");
}

QString TraceRecord::duration() const {
    return sample_.is_value_Set() ? formatDuration(sample_.getValue()) : QString{};
}

QString TraceRecord::count() const {
    return sample_.is_value_Set() ? QLocale().toString(sample_.getValue()) : QString{};
}

bool TraceRecord::failed() const {
    return sample_.getSpanOutcome().getValue() ==
           openapi::DiagnosticSpanOutcome::eDiagnosticSpanOutcome::FAILED;
}

bool TraceRecord::succeeded() const {
    return sample_.getSpanOutcome().getValue() ==
           openapi::DiagnosticSpanOutcome::eDiagnosticSpanOutcome::SUCCEEDED;
}

QString TraceRecord::httpRequest() const {
    if (!sample_.is_http_method_Set()) {
        return {};
    }
    const auto route = sample_.getHttpRoute();
    return QStringLiteral("%1 %2").arg(sample_.getHttpMethod(),
                                       route.isEmpty() ? tr("Unmatched route") : route);
}

QString TraceRecord::httpStatus() const {
    return sample_.is_http_status_Set() ? QString::number(sample_.getHttpStatus()) : QString{};
}

QString TraceRecord::observedAt() const {
    const auto instant = QDateTime::fromSecsSinceEpoch(sample_.getTimestamp());
    return QLocale().toString(instant.date(), QLocale::ShortFormat) + ' ' +
           instant.time().toString(QStringLiteral("HH:mm:ss"));
}

QString TraceRecord::timestamp() const {
    const auto micros = sample_.getStartedAtUnixMicros();
    const auto seconds = hasStart() ? micros / MicrosPerSecond : sample_.getTimestamp();
    const auto instant = QDateTime::fromSecsSinceEpoch(seconds).toLocalTime();
    auto result = QLocale().toString(instant.date(), QLocale::ShortFormat) + ' ' +
                  instant.time().toString(QStringLiteral("HH:mm:ss"));
    if (hasStart()) {
        result += QStringLiteral(".%1").arg(micros % MicrosPerSecond, 6, 10, QLatin1Char('0'));
    }
    return result;
}

QStringList TraceRecord::events() const {
    QStringList result;
    for (const auto &event : sample_.getEvents()) {
        result.append(QStringLiteral("%1 · %2").arg(event.getName(),
                                                    formatDuration(event.getOffsetMicros())));
    }
    return result;
}

} // namespace ctxfab
