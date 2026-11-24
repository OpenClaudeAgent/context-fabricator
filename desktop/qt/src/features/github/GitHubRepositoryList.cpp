#include "GitHubRepositoryList.h"

#include <algorithm>
#include <utility>

namespace ctxfab {

int GitHubRepositoryList::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(entries_.size());
}

QVariant GitHubRepositoryList::data(const QModelIndex &index, int role) const {
    if (role != EntryRole || !checkIndex(index, CheckIndexOption::IndexIsValid)) {
        return {};
    }
    return QVariant::fromValue(entries_.at(index.row()));
}

QHash<int, QByteArray> GitHubRepositoryList::roleNames() const { return {{EntryRole, "entry"}}; }

QStringList GitHubRepositoryList::names() const {
    QStringList names;
    names.reserve(entries_.size());
    for (const auto &entry : entries_) {
        names.append(entry.name());
    }
    return names;
}

void GitHubRepositoryList::replace(QList<GitHubRepositoryEntry> entries) {
    if (!std::ranges::equal(entries_, entries, {}, &GitHubRepositoryEntry::id,
                            &GitHubRepositoryEntry::id)) {
        beginResetModel();
        entries_ = std::move(entries);
        endResetModel();
        emit namesChanged();
        return;
    }
    bool renamed = false;
    for (int row = 0; row < rowCount(); ++row) {
        if (entries_.at(row) != entries.at(row)) {
            renamed = renamed || entries_.at(row).name() != entries.at(row).name();
            entries_[row] = std::move(entries[row]);
            emit dataChanged(index(row), index(row), {EntryRole});
        }
    }
    if (renamed) {
        emit namesChanged();
    }
}

} // namespace ctxfab
