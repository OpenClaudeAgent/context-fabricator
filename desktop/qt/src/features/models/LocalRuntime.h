#pragma once

#include "LocalApiRequest.h"
#include "RuntimeStatus.h"

#include <QTimer>
#include <QUrl>
#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

class BackendSupervisor;

class LocalRuntime final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool hasRuntime READ hasRuntime NOTIFY changed FINAL)
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool canDownload READ canDownload NOTIFY changed FINAL)
    Q_PROPERTY(bool canImport READ canImport NOTIFY changed FINAL)
    Q_PROPERTY(bool needsRestart READ needsRestart NOTIFY changed FINAL)
    Q_PROPERTY(QString engine READ engine NOTIFY changed FINAL)
    Q_PROPERTY(QString status READ status NOTIFY changed FINAL)
    Q_PROPERTY(QString archiveSummary READ archiveSummary NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)

  public:
    explicit LocalRuntime(BackendSupervisor &backend);
    bool hasRuntime() const;
    bool ready() const { return runtime_.getState() == QStringLiteral("ready"); }
    bool busy() const;
    bool canDownload() const;
    bool canImport() const;
    bool needsRestart() const;
    QString engine() const;
    QString status() const;
    QString archiveSummary() const;
    QString feedback() const;

    Q_INVOKABLE void setObserved(bool observed);
    Q_INVOKABLE void download();
    Q_INVOKABLE void install(const QUrl &archive);

  signals:
    void changed();

  private:
    enum class Request { None, Observe, Download, Import };
    void reset();
    void refresh();
    void send(Request request, const QString &archivePath = {});
    void receive(const openapi::HttpRequestWorker &worker, const openapi::RuntimeStatus &status);
    void reject(const openapi::HttpRequestWorker &worker);
    bool canInstall() const;
    bool issuing() const;

    BackendSupervisor &backend_;
    QTimer poll_;
    LocalApiRequest request_;
    openapi::RuntimeStatus runtime_;
    QString readError_;
    QString commandError_;
    Request pending_ = Request::None;
};

} // namespace ctxfab
