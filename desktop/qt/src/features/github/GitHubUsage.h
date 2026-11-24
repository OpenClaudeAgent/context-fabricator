#pragma once

#include "GitHubStorageBudget.h"
#include "GitHubTraffic.h"

#include <QCoreApplication>
#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

class GitHubUsage {
    Q_GADGET
    QML_VALUE_TYPE(gitHubUsage)
    Q_DECLARE_TR_FUNCTIONS(GitHubUsage)

    Q_PROPERTY(bool hasTraffic READ hasTraffic CONSTANT FINAL)
    Q_PROPERTY(int effectiveRate READ effectiveRate CONSTANT FINAL)
    Q_PROPERTY(int maximumRate READ maximumRate CONSTANT FINAL)
    Q_PROPERTY(int requestedRate READ requestedRate CONSTANT FINAL)
    Q_PROPERTY(QString concurrency READ concurrency CONSTANT FINAL)
    Q_PROPERTY(QString attempts READ attempts CONSTANT FINAL)
    Q_PROPERTY(QString failures READ failures CONSTANT FINAL)
    Q_PROPERTY(QString queued READ queued CONSTANT FINAL)
    Q_PROPERTY(QString inFlight READ inFlight CONSTANT FINAL)
    Q_PROPERTY(QString uncertain READ uncertain CONSTANT FINAL)
    Q_PROPERTY(QString cooldown READ cooldown CONSTANT FINAL)
    Q_PROPERTY(int storageLimit READ storageLimit CONSTANT FINAL)
    Q_PROPERTY(QString retainedPayload READ retainedPayload CONSTANT FINAL)

  public:
    GitHubUsage() = default;
    GitHubUsage(const openapi::GitHubTraffic &traffic, const openapi::GitHubStorageBudget &budget);

    [[nodiscard]] bool hasTraffic() const { return traffic_.isValid(); }
    [[nodiscard]] int effectiveRate() const { return traffic_.getEffectiveRequestsPerMinute(); }
    [[nodiscard]] int maximumRate() const {
        return traffic_.getMaximumConfigurableRequestsPerMinute();
    }
    [[nodiscard]] int requestedRate() const;
    [[nodiscard]] QString concurrency() const;
    [[nodiscard]] QString attempts() const;
    [[nodiscard]] QString failures() const;
    [[nodiscard]] QString queued() const;
    [[nodiscard]] QString inFlight() const;
    [[nodiscard]] QString uncertain() const;
    [[nodiscard]] QString cooldown() const;
    [[nodiscard]] int storageLimit() const { return budget_.getLimitGb(); }
    [[nodiscard]] qint64 bytes() const { return budget_.getBytes(); }
    [[nodiscard]] QString retainedPayload() const;

  private:
    openapi::GitHubTraffic traffic_;
    openapi::GitHubStorageBudget budget_;
};

} // namespace ctxfab
