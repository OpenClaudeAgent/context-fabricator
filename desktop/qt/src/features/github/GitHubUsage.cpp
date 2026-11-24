#include "GitHubUsage.h"

#include <QDateTime>
#include <QLocale>

#include <algorithm>

namespace ctxfab {

GitHubUsage::GitHubUsage(const openapi::GitHubTraffic &traffic,
                         const openapi::GitHubStorageBudget &budget)
    : traffic_(traffic), budget_(budget) {}

int GitHubUsage::requestedRate() const {
    return traffic_.is_requested_requests_per_minute_Set()
               ? traffic_.getRequestedRequestsPerMinute()
               : std::min(effectiveRate(), maximumRate());
}

QString GitHubUsage::concurrency() const { return QLocale().toString(traffic_.getConcurrency()); }
QString GitHubUsage::attempts() const { return QLocale().toString(traffic_.getAttempts()); }
QString GitHubUsage::failures() const { return QLocale().toString(traffic_.getFailures()); }
QString GitHubUsage::queued() const { return QLocale().toString(traffic_.getQueued()); }
QString GitHubUsage::inFlight() const { return QLocale().toString(traffic_.getPending()); }
QString GitHubUsage::uncertain() const { return QLocale().toString(traffic_.getUncertain()); }

QString GitHubUsage::cooldown() const {
    return traffic_.is_cooldown_until_ms_Set()
               ? QLocale().toString(QDateTime::fromMSecsSinceEpoch(traffic_.getCooldownUntilMs()),
                                    QLocale::ShortFormat)
               : QString{};
}

QString GitHubUsage::retainedPayload() const {
    const QLocale locale;
    return tr("%1 GB (%2 bytes)")
        .arg(locale.toString(static_cast<double>(bytes()) / 1'000'000'000.0, 'f', 3),
             locale.toString(bytes()));
}

} // namespace ctxfab
