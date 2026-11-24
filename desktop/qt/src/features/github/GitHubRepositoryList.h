#pragma once

#include "GitHubRepositoryEntry.h"

#include <QAbstractListModel>

namespace ctxfab {

class GitHubRepositoryList final : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by the GitHub connector")
    Q_PROPERTY(int count READ count NOTIFY namesChanged FINAL)
    Q_PROPERTY(QStringList names READ names NOTIFY namesChanged FINAL)

  public:
    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;
    int count() const { return rowCount(); }
    [[nodiscard]] QStringList names() const;
    GitHubRepositoryEntry entryAt(int row) const { return entries_.value(row); }
    void replace(QList<GitHubRepositoryEntry> entries);

  signals:
    void namesChanged();

  private:
    static constexpr int EntryRole = Qt::UserRole + 1;
    QList<GitHubRepositoryEntry> entries_;
};

} // namespace ctxfab
