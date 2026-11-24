#pragma once

#include "ModelDownload.h"
#include "ModelStoragePlan.h"

#include <QCoreApplication>
#include <QUrl>
#include <QtQmlIntegration/qqmlintegration.h>

namespace ctxfab {

QString formatArtifactSize(qint64 bytes);

class LibraryModel final {
    Q_GADGET
    QML_VALUE_TYPE(libraryModel)
    Q_DECLARE_TR_FUNCTIONS(LibraryModel)

  public:
    enum Action { NoAction, Download, Pause, Resume, Remove };

    Q_PROPERTY(QString profile READ profile CONSTANT FINAL)
    Q_PROPERTY(QString label READ label CONSTANT FINAL)
    Q_PROPERTY(QString status READ status CONSTANT FINAL)
    Q_PROPERTY(QString source READ source CONSTANT FINAL)
    Q_PROPERTY(QUrl sourceUrl READ sourceUrl CONSTANT FINAL)
    Q_PROPERTY(QString feedback READ feedback CONSTANT FINAL)
    Q_PROPERTY(QString peakSpace READ peakSpace CONSTANT FINAL)
    Q_PROPERTY(QString actionLabel READ actionLabel CONSTANT FINAL)
    Q_PROPERTY(bool canAct READ canAct CONSTANT FINAL)
    Q_PROPERTY(bool canDiscard READ canDiscard CONSTANT FINAL)
    Q_PROPERTY(bool transferring READ transferring CONSTANT FINAL)
    Q_PROPERTY(bool determinate READ determinate CONSTANT FINAL)
    Q_PROPERTY(double progress READ progress CONSTANT FINAL)
    Q_PROPERTY(QString partialSize READ partialSize CONSTANT FINAL)
    Q_PROPERTY(QString size READ size CONSTANT FINAL)
    Q_PROPERTY(bool recommendedForSetup READ recommendedForSetup CONSTANT FINAL)

    LibraryModel() = default;
    LibraryModel(const openapi::ModelDownload &download, const openapi::ModelStoragePlan &storage);

    static bool accepts(const openapi::ModelDownload &download);
    QString profile() const { return download_.getProfile(); }
    QString kind() const { return download_.getKind(); }
    QString label() const;
    QString status() const;
    QString source() const { return download_.getSource().getLabel(); }
    QUrl sourceUrl() const;
    QString feedback() const;
    QString peakSpace() const;
    Action action() const;
    QString actionLabel() const;
    bool canAct() const { return action() != NoAction; }
    bool canDiscard() const { return action() == Resume; }
    bool transferring() const;
    bool determinate() const;
    double progress() const;
    QString partialSize() const { return formatArtifactSize(downloadedBytes()); }
    QString size() const { return formatArtifactSize(totalBytes()); }
    QString setupRole() const { return download_.getSetupRole(); }
    bool recommendedForSetup() const { return setupRole() == QStringLiteral("recommendedAnswer"); }
    qint64 downloadedBytes() const { return download_.getDownloadedBytes(); }
    qint64 totalBytes() const { return download_.getTotalBytes(); }
    bool installed() const;
    bool inDownloads() const;
    bool matches(const QString &query) const;
    bool operator==(const LibraryModel &other) const;

  private:
    enum class State {
        Unknown,
        NotInstalled,
        Installing,
        Pausing,
        Paused,
        Interrupted,
        Installed,
        Failed
    };
    static State stateOf(const QString &value);
    QString installingStatus(const QString &done, const QString &total) const;

    openapi::ModelDownload download_;
    openapi::ModelStoragePlan storage_;
    State state_ = State::Unknown;
};

} // namespace ctxfab
