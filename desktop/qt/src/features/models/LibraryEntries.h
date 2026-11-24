#pragma once

#include "LibraryModel.h"

#include <QAbstractListModel>

namespace ctxfab {

class LibraryEntries final : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by the model library")
    Q_PROPERTY(int count READ count NOTIFY countChanged FINAL)

  public:
    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;
    int count() const { return rowCount(); }
    Q_INVOKABLE LibraryModel entryAt(int row) const { return entries_.value(row); }
    Q_INVOKABLE bool containsProfile(const QString &profile) const;
    void replace(QList<LibraryModel> entries);
    void retranslate();

  signals:
    void countChanged();

  private:
    static constexpr int EntryRole = Qt::UserRole + 1;
    QList<LibraryModel> entries_;
};

} // namespace ctxfab
