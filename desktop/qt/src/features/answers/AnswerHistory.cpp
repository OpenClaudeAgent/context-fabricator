#include "AnswerHistory.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "HistoryPreferences.h"

#include <QDateTime>

#include <algorithm>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
constexpr int EntryRole = Qt::UserRole + 1;
constexpr int PageSize = 25;
constexpr int MaximumRows = 250;
constexpr qint64 ExpiryCheckIntervalMs = 3'600'000;

bool orderedRows(const QList<openapi::AnswerHistorySummary> &rows, qint64 before) {
    for (const auto &row : rows) {
        const auto id = row.getId();
        if (!row.isValid() || id <= 0 || (before != 0 && id >= before)) {
            return false;
        }
        before = id;
    }
    return true;
}

bool validPage(const openapi::AnswerHistoryPage &page, qint64 before) {
    const auto entries = page.getEntries();
    return page.isValid() && entries.size() <= PageSize && orderedRows(entries, before) &&
           (!page.is_next_before_id_Set() ||
            (!entries.isEmpty() && page.getNextBeforeId() == entries.constLast().getId()));
}

} // namespace

AnswerHistory::AnswerHistory(BackendSupervisor &backend, HistoryPreferences &preferences)
    : backend_(backend), preferences_(preferences) {
    expiryTimer_.setSingleShot(true);
    connect(&expiryTimer_, &QTimer::timeout, this, &AnswerHistory::checkExpiry);
    connect(&backend_, &BackendSupervisor::changed, this, &AnswerHistory::backendChanged);
    connect(&preferences_, &HistoryPreferences::changed, this, &AnswerHistory::policyChanged);
    connect(&preferences_, &HistoryPreferences::historyCleared, this, &AnswerHistory::invalidate);
}

int AnswerHistory::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(entries_.size());
}

QVariant AnswerHistory::data(const QModelIndex &index, int role) const {
    if (index.isValid() && role == EntryRole && index.row() < rowCount()) {
        return QVariant::fromValue(HistorySummary(entries_.at(index.row())));
    }
    return {};
}

QHash<int, QByteArray> AnswerHistory::roleNames() const { return {{EntryRole, "entry"}}; }

bool AnswerHistory::savingEnabled() const { return preferences_.ready() && preferences_.enabled(); }

QString AnswerHistory::feedback() const {
    if (!detailFeedback_.isEmpty()) {
        return tr(qUtf8Printable(detailFeedback_));
    }
    if (!pageFeedback_.isEmpty()) {
        return tr(qUtf8Printable(pageFeedback_));
    }
    return preferences_.ready() ? QString{} : preferences_.feedback();
}

QString AnswerHistory::selectedId() const {
    return selectedId_ == 0 ? QString{} : QString::number(selectedId_);
}

void AnswerHistory::retranslate() {
    if (!entries_.isEmpty()) {
        emit dataChanged(index(0), index(rowCount() - 1), {EntryRole});
    }
    emit changed();
}

void AnswerHistory::open() {
    if (!open_) {
        open_ = true;
        awaitingPolicy_ = true;
        preferences_.reload();
    }
}

void AnswerHistory::close() {
    open_ = false;
    awaitingPolicy_ = false;
    discard();
}

void AnswerHistory::refresh() {
    if (open_ && !awaitingPolicy_ && savingEnabled()) {
        loadPage(0);
    }
}

void AnswerHistory::loadMore() {
    if (open_ && !loading() && hasMore() && savingEnabled()) {
        loadPage(beforeId_);
    }
}

void AnswerHistory::invalidate() {
    discard();
    refresh();
}

void AnswerHistory::backendChanged() {
    awaitingPolicy_ = open_ && backend_.session().has_value();
    discard();
}

void AnswerHistory::policyChanged() {
    if (preferences_.busy()) {
        emit changed();
        return;
    }
    const bool enabled = savingEnabled();
    const auto days = preferences_.retentionDays();
    const bool reduced = retentionDays_ > days;
    const bool reload = awaitingPolicy_ || !savingWasEnabled_ || reduced;
    awaitingPolicy_ = false;
    savingWasEnabled_ = enabled;
    retentionDays_ = days;
    if (!enabled || reduced) {
        discard();
    }
    if (reload) {
        refresh();
    }
    emit changed();
}

void AnswerHistory::discard() {
    listRequest_.cancel();
    clearDetail();
    pageLoading_ = false;
    pageFeedback_.clear();
    beforeId_ = 0;
    if (!entries_.isEmpty()) {
        beginResetModel();
        entries_.clear();
        endResetModel();
    }
    emit changed();
}

void AnswerHistory::loadPage(qint64 before) {
    const auto session = backend_.session();
    if (!session) {
        finishPage(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    auto *api = listRequest_.begin(*session);
    connect(api, &Api::listAnswerHistorySignalFull, this,
            [this, before](const Worker *worker, const openapi::AnswerHistoryPage &page) {
                receivePage(*worker, page, before);
            });
    connect(api, &Api::listAnswerHistorySignalErrorFull, this, [this](const Worker *worker) {
        finishPage(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    pageLoading_ = true;
    pageFeedback_.clear();
    api->listAnswerHistory(before == 0 ? openapi::OptionalParam<qint64>{}
                                       : openapi::OptionalParam<qint64>{before});
    emit changed();
}

void AnswerHistory::receivePage(const Worker &worker, const openapi::AnswerHistoryPage &page,
                                qint64 before) {
    if (worker.getHttpResponseCode() != 200 || !validPage(page, before)) {
        finishPage(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    beforeId_ = page.is_next_before_id_Set() ? page.getNextBeforeId() : 0;
    if (before == 0) {
        beginResetModel();
        entries_ = page.getEntries();
        endResetModel();
    } else {
        appendPage(page.getEntries());
    }
    finishPage({});
}

void AnswerHistory::appendPage(const QList<openapi::AnswerHistorySummary> &rows) {
    if (rows.isEmpty()) {
        return;
    }
    const int count = static_cast<int>(rows.size());
    const int drop = rowCount() + count - MaximumRows;
    if (drop > 0) {
        beginRemoveRows({}, 0, drop - 1);
        entries_.remove(0, drop);
        endRemoveRows();
    }
    beginInsertRows({}, rowCount(), rowCount() + count - 1);
    entries_.append(rows);
    endInsertRows();
}

void AnswerHistory::finishPage(const QString &message) {
    listRequest_.cancel();
    pageLoading_ = false;
    pageFeedback_ = message;
    emit changed();
}

void AnswerHistory::clearDetail() {
    detailRequest_.cancel();
    expiryTimer_.stop();
    savedAnswer_ = {};
    selectedId_ = 0;
    expiresAtMs_ = 0;
    detailLoading_ = false;
    detailFeedback_.clear();
}

void AnswerHistory::deselect() {
    clearDetail();
    emit changed();
}

void AnswerHistory::select(int row) {
    const auto session = backend_.session();
    if (!open_ || !savingEnabled() || !session || row < 0 || row >= rowCount()) {
        return;
    }
    clearDetail();
    selectedId_ = entries_.at(row).getId();
    detailLoading_ = true;
    auto *api = detailRequest_.begin(*session);
    connect(api, &Api::getAnswerHistoryEntrySignalFull, this,
            [this](const Worker *worker, const openapi::AnswerHistoryEntry &entry) {
                receiveDetail(*worker, entry);
            });
    connect(api, &Api::getAnswerHistoryEntrySignalErrorFull, this, [this](const Worker *worker) {
        finishDetail(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    api->getAnswerHistoryEntry(selectedId_);
    emit changed();
}

void AnswerHistory::receiveDetail(const Worker &worker, const openapi::AnswerHistoryEntry &entry) {
    if (worker.getHttpResponseCode() != 200 || !entry.isValid() || entry.getId() != selectedId_) {
        finishDetail(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    const SavedAnswer candidate(entry);
    expiresAtMs_ = candidate.expiresAtMs(retentionDays_);
    if (expiresAtMs_ <= QDateTime::currentMSecsSinceEpoch()) {
        finishDetail(QT_TR_NOOP("Saved Answer Unavailable"));
        return;
    }
    savedAnswer_ = candidate;
    checkExpiry();
    finishDetail({});
}

void AnswerHistory::finishDetail(const QString &message) {
    detailRequest_.cancel();
    detailLoading_ = false;
    detailFeedback_ = message;
    emit changed();
}

void AnswerHistory::checkExpiry() {
    const auto delay = expiresAtMs_ - QDateTime::currentMSecsSinceEpoch();
    if (delay > 0) {
        expiryTimer_.start(static_cast<int>(std::min(delay, ExpiryCheckIntervalMs)));
    } else {
        deselect();
        refresh();
    }
}

} // namespace ctxfab
