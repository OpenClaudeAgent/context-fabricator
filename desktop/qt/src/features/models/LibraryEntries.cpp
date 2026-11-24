#include "LibraryEntries.h"

#include <algorithm>
#include <utility>

namespace ctxfab {

int LibraryEntries::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(entries_.size());
}

QVariant LibraryEntries::data(const QModelIndex &index, int role) const {
    if (role != EntryRole || !checkIndex(index, CheckIndexOption::IndexIsValid)) {
        return {};
    }
    return QVariant::fromValue(entries_.at(index.row()));
}

QHash<int, QByteArray> LibraryEntries::roleNames() const { return {{EntryRole, "entry"}}; }

bool LibraryEntries::containsProfile(const QString &profile) const {
    return std::ranges::find(entries_, profile, &LibraryModel::profile) != entries_.end();
}

void LibraryEntries::replace(QList<LibraryModel> entries) {
    if (!std::ranges::equal(entries_, entries, {}, &LibraryModel::profile,
                            &LibraryModel::profile)) {
        beginResetModel();
        entries_ = std::move(entries);
        endResetModel();
        emit countChanged();
        return;
    }
    for (int row = 0; row < rowCount(); ++row) {
        if (entries_.at(row) != entries.at(row)) {
            entries_[row] = std::move(entries[row]);
            emit dataChanged(index(row), index(row), {EntryRole});
        }
    }
}

void LibraryEntries::retranslate() {
    if (!entries_.isEmpty()) {
        // The payload is unchanged, but the delegates must reread its translated properties.
        beginResetModel();
        endResetModel();
    }
}

} // namespace ctxfab
