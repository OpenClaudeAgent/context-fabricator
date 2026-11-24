#pragma once

#include "DiagnosticEvent.h"
#include "LocalApiRequest.h"
#include "TraceRecord.h"

#include <QAbstractListModel>
#include <QTimer>
#include <QtQmlIntegration/qqmlintegration.h>

#include <optional>

namespace ctxfab {

class BackendSupervisor;
namespace openapi {
class DiagnosticLogPage;
}

class DiagnosticLogs final : public QAbstractListModel {
    Q_OBJECT
    QML_NAMED_ELEMENT(DiagnosticLogs)
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool hasMore READ hasMore NOTIFY changed FINAL)
    Q_PROPERTY(QString query READ query NOTIFY changed FINAL)
    Q_PROPERTY(int level READ level NOTIFY changed FINAL)
    Q_PROPERTY(bool allLevels READ allLevels NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::DiagnosticEvent> events READ events NOTIFY eventsChanged FINAL)

  public:
    enum class Feed { ApplicationLogs, AnswerRequests };
    enum Level { AllLevels, Info, Error, Debug };
    static constexpr int SampleRole = Qt::UserRole + 1;
    explicit DiagnosticLogs(BackendSupervisor &backend, Feed feed = Feed::ApplicationLogs);

    [[nodiscard]] int rowCount(const QModelIndex &parent = {}) const override;
    [[nodiscard]] QVariant data(const QModelIndex &index, int role) const override;
    [[nodiscard]] QHash<int, QByteArray> roleNames() const override;
    [[nodiscard]] bool ready() const { return ready_; }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] bool hasMore() const { return before_.has_value(); }
    [[nodiscard]] const QString &query() const { return query_; }
    [[nodiscard]] int level() const { return level_; }
    [[nodiscard]] bool allLevels() const { return level_ == AllLevels; }
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] QList<DiagnosticEvent> events() const;
    void retranslate();

    Q_INVOKABLE void setActive(bool active);
    Q_INVOKABLE void setLevel(int level);
    Q_INVOKABLE void setQuery(const QString &query);
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void loadMore();

  signals:
    void changed();
    void eventsChanged();

  private:
    void backendChanged();
    void resetPage();
    void cancelPage();
    void loadPage(std::optional<qint64> before = {});
    void acceptPage(const openapi::HttpRequestWorker &worker,
                    const openapi::DiagnosticLogPage &page, std::optional<qint64> before);
    void finishPage(const QString &message);
    void loadEvents();
    void finishEvents(const QString &message);

    BackendSupervisor &backend_;
    const Feed feed_;
    LocalApiRequest pageRequest_;
    LocalApiRequest eventRequest_;
    QTimer debounce_;
    QList<TraceRecord> records_;
    QList<DiagnosticEvent> events_;
    std::optional<qint64> before_;
    QString query_;
    QString pageFeedback_;
    QString eventsFeedback_;
    Level level_ = AllLevels;
    bool active_ = false;
    bool ready_ = false;
    bool busy_ = false;
};

} // namespace ctxfab
