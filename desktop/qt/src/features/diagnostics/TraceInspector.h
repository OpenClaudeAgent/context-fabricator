#pragma once

#include "LocalApiRequest.h"
#include "TraceRecord.h"

#include <QObject>
#include <QTimer>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class BackendSupervisor;

namespace openapi {
class DiagnosticTracePage;
}

class TraceInspector final : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(TraceInspector)
    QML_UNCREATABLE("Created by the desktop application")

    Q_PROPERTY(bool visible READ visible NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool hasMore READ hasMore NOTIFY changed FINAL)
    Q_PROPERTY(bool canGoBack READ canGoBack NOTIFY changed FINAL)
    Q_PROPERTY(QString traceId READ traceId NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(bool opensRoot READ opensRoot NOTIFY destinationChanged FINAL)
    Q_PROPERTY(bool recordedAbstention READ recordedAbstention NOTIFY recordsChanged FINAL)
    Q_PROPERTY(QString abstentionDetail READ abstentionDetail NOTIFY recordsChanged FINAL)
    Q_PROPERTY(QList<ctxfab::TraceDetailField> recordedRetrieval READ recordedRetrieval NOTIFY
                   recordsChanged FINAL)
    Q_PROPERTY(QList<ctxfab::TraceRecord> spans READ spans NOTIFY recordsChanged FINAL)
    Q_PROPERTY(QList<ctxfab::TraceRecord> logs READ logs NOTIFY recordsChanged FINAL)
    Q_PROPERTY(QList<ctxfab::TraceRecord> spanLogs READ spanLogs NOTIFY selectionChanged FINAL)
    Q_PROPERTY(QStringList spanTitles READ spanTitles NOTIFY recordsChanged FINAL)
    Q_PROPERTY(int selectedIndex READ selectedIndex NOTIFY selectionChanged FINAL)
    Q_PROPERTY(ctxfab::TraceRecord selected READ selected NOTIFY selectionChanged FINAL)

  public:
    explicit TraceInspector(BackendSupervisor &backend);

    [[nodiscard]] bool visible() const { return !destination_.trace.isEmpty(); }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] bool hasMore() const { return beforeId_ > 0; }
    [[nodiscard]] bool canGoBack() const { return !ancestry_.isEmpty(); }
    [[nodiscard]] QString traceId() const { return destination_.trace; }
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] bool opensRoot() const { return destination_.span.isEmpty(); }
    [[nodiscard]] bool isAnswerTrace() const;
    [[nodiscard]] bool recordedAbstention() const;
    [[nodiscard]] QString abstentionDetail() const;
    [[nodiscard]] QList<TraceDetailField> recordedRetrieval() const;
    [[nodiscard]] QList<TraceRecord> spans() const { return outline_; }
    [[nodiscard]] QList<TraceRecord> logs() const { return logs_; }
    [[nodiscard]] QList<TraceRecord> spanLogs() const;
    [[nodiscard]] QStringList spanTitles() const;
    [[nodiscard]] int selectedIndex() const;
    [[nodiscard]] TraceRecord selected() const { return outline_.value(selectedIndex()); }

    Q_INVOKABLE void open(const QString &trace, const QString &span);
    Q_INVOKABLE void close();
    Q_INVOKABLE void reload();
    Q_INVOKABLE void loadMore();
    Q_INVOKABLE void selectSpan(int index);
    Q_INVOKABLE void followWorkTrace();
    Q_INVOKABLE void goBack();
    Q_INVOKABLE void copyTraceId();

  signals:
    void changed();
    void destinationChanged();
    void recordsChanged();
    void selectionChanged();

  private:
    struct Destination {
        QString trace;
        QString span;
    };

    void openDestination(Destination destination);
    void resetRecords();
    void load(qint64 before, bool canRetry);
    void receive(const openapi::HttpRequestWorker &worker, const openapi::DiagnosticTracePage &page,
                 qint64 before, bool canRetry);
    void publish(const openapi::DiagnosticTracePage &page);
    void finish(const QString &feedback = {});

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    QTimer retry_;
    Destination destination_;
    QList<Destination> ancestry_;
    QList<openapi::SignalSample> spanSamples_;
    QList<TraceRecord> outline_;
    QList<TraceRecord> logs_;
    QString feedback_;
    qint64 selectedId_ = 0;
    qint64 beforeId_ = 0;
    bool busy_ = false;
};

} // namespace ctxfab
