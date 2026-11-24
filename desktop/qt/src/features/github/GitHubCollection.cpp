#include "GitHubCollection.h"

#include <QDateTime>
#include <QLocale>

#include <algorithm>
#include <array>
#include <limits>

namespace ctxfab {
namespace {

constexpr int ActiveDelay = 2000;
constexpr int OverdueDelay = 10000;

struct CollectionLabel {
    QLatin1StringView phase;
    const char *label;
};

constexpr std::array Labels{
    CollectionLabel{QLatin1StringView("disabled"),
                    QT_TRANSLATE_NOOP("GitHubCollection", "Collection off")},
    CollectionLabel{QLatin1StringView("credentials"),
                    QT_TRANSLATE_NOOP("GitHubCollection", "Read access required")},
    CollectionLabel{QLatin1StringView("queued"), QT_TRANSLATE_NOOP("GitHubCollection", "Queued")},
    CollectionLabel{QLatin1StringView("succeeded"),
                    QT_TRANSLATE_NOOP("GitHubCollection", "Up to date")},
    CollectionLabel{QLatin1StringView("retry"),
                    QT_TRANSLATE_NOOP("GitHubCollection", "Retry scheduled")},
    CollectionLabel{QLatin1StringView("throttled"),
                    QT_TRANSLATE_NOOP("GitHubCollection", "Waiting for GitHub")},
    CollectionLabel{QLatin1StringView("failed"),
                    QT_TRANSLATE_NOOP("GitHubCollection", "Refresh failed")},
};

QString formattedDate(qint64 timestamp) {
    return QLocale().toString(QDateTime::fromMSecsSinceEpoch(timestamp), QLocale::ShortFormat);
}

} // namespace

GitHubCollection::GitHubCollection(const openapi::GitHubCodeCollection &snapshot, bool graphOnly)
    : snapshot_(snapshot), graphOnly_(graphOnly) {}

bool GitHubCollection::running() const {
    return snapshot_.getPhase() == QLatin1StringView("running");
}

QString GitHubCollection::status() const {
    if (running()) {
        return snapshot_.getPending().getPhase() == QLatin1StringView("manifestComplete")
                   ? tr("Preparing index")
                   : tr("Syncing files");
    }
    const auto phase = snapshot_.getPhase();
    const auto label = std::ranges::find_if(
        Labels, [&phase](const CollectionLabel &entry) { return phase == entry.phase; });
    return label != Labels.end() ? tr(label->label) : QString{};
}

QString GitHubCollection::pendingCommit() const { return snapshot_.getPending().getCommit(); }

int GitHubCollection::pendingFiles() const {
    return snapshot_.is_pending_Set() ? snapshot_.getPending().getStagedFiles() : -1;
}

int GitHubCollection::publishedFiles() const {
    return snapshot_.is_published_files_Set() ? snapshot_.getPublishedFiles() : -1;
}

QString GitHubCollection::lastChecked() const {
    return snapshot_.is_last_checked_at_ms_Set() && !running()
               ? formattedDate(snapshot_.getLastCheckedAtMs())
               : QString{};
}

QString GitHubCollection::nextCheck() const {
    const auto phase = snapshot_.getPhase();
    const bool scheduled = phase == QLatin1StringView("succeeded") ||
                           phase == QLatin1StringView("throttled") ||
                           phase == QLatin1StringView("retry");
    return scheduled && snapshot_.is_next_at_ms_Set() ? formattedDate(snapshot_.getNextAtMs())
                                                      : QString{};
}

QString GitHubCollection::error() const {
    const auto phase = snapshot_.getPhase();
    return phase == QLatin1StringView("failed") || phase == QLatin1StringView("retry")
               ? snapshot_.getError()
               : QString{};
}

int GitHubCollection::pollDelay(qint64 now) const {
    if (running() || snapshot_.getPhase() == QLatin1StringView("queued")) {
        return ActiveDelay;
    }
    if (!snapshot_.is_next_at_ms_Set()) {
        return -1;
    }
    const auto wait = snapshot_.getNextAtMs() - now;
    constexpr qint64 MaxWait = std::numeric_limits<int>::max() - ActiveDelay;
    return wait > 0 ? static_cast<int>(std::min(wait, MaxWait)) + ActiveDelay : OverdueDelay;
}

} // namespace ctxfab
