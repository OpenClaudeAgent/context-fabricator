#pragma once

#include "LocalApiRequest.h"
#include "SavedAnswer.h"

#include <QAbstractListModel>
#include <QTimer>
#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

class BackendSupervisor;
class HistoryPreferences;

namespace openapi {
class AnswerHistoryPage;
}

class AnswerHistory final : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")

    Q_PROPERTY(bool savingEnabled READ savingEnabled NOTIFY changed FINAL)
    Q_PROPERTY(bool loading READ loading NOTIFY changed FINAL)
    Q_PROPERTY(bool loadingDetail READ loadingDetail NOTIFY changed FINAL)
    Q_PROPERTY(bool hasMore READ hasMore NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString selectedId READ selectedId NOTIFY changed FINAL)
    Q_PROPERTY(ctxfab::SavedAnswer savedAnswer READ savedAnswer NOTIFY changed FINAL)

  public:
    AnswerHistory(BackendSupervisor &backend, HistoryPreferences &preferences);

    [[nodiscard]] int rowCount(const QModelIndex &parent = {}) const override;
    [[nodiscard]] QVariant data(const QModelIndex &index, int role) const override;
    [[nodiscard]] QHash<int, QByteArray> roleNames() const override;
    [[nodiscard]] bool savingEnabled() const;
    [[nodiscard]] bool loading() const { return awaitingPolicy_ || pageLoading_; }
    [[nodiscard]] bool loadingDetail() const { return detailLoading_; }
    [[nodiscard]] bool hasMore() const { return beforeId_ != 0; }
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] QString selectedId() const;
    [[nodiscard]] SavedAnswer savedAnswer() const { return savedAnswer_; }

    Q_INVOKABLE void open();
    Q_INVOKABLE void close();
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void loadMore();
    Q_INVOKABLE void select(int row);
    Q_INVOKABLE void deselect();
    void invalidate();
    void retranslate();

  signals:
    void changed();

  private:
    void backendChanged();
    void policyChanged();
    void discard();
    void clearDetail();
    void loadPage(qint64 before);
    void receivePage(const openapi::HttpRequestWorker &worker,
                     const openapi::AnswerHistoryPage &page, qint64 before);
    void appendPage(const QList<openapi::AnswerHistorySummary> &rows);
    void finishPage(const QString &message);
    void receiveDetail(const openapi::HttpRequestWorker &worker,
                       const openapi::AnswerHistoryEntry &entry);
    void finishDetail(const QString &message);
    void checkExpiry();

    BackendSupervisor &backend_;
    HistoryPreferences &preferences_;
    LocalApiRequest listRequest_;
    LocalApiRequest detailRequest_;
    QTimer expiryTimer_;
    QList<openapi::AnswerHistorySummary> entries_;
    SavedAnswer savedAnswer_;
    QString pageFeedback_;
    QString detailFeedback_;
    qint64 selectedId_ = 0;
    qint64 beforeId_ = 0;
    qint64 expiresAtMs_ = 0;
    int retentionDays_ = 0;
    bool savingWasEnabled_ = false;
    bool open_ = false;
    bool awaitingPolicy_ = false;
    bool pageLoading_ = false;
    bool detailLoading_ = false;
};

} // namespace ctxfab
