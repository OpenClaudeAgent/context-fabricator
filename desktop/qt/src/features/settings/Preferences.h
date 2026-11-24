#pragma once

#include "LocalApiRequest.h"
#include "Settings.h"

#include <QObject>
#include <QStringList>
#include <QtQmlIntegration/qqmlintegration.h>

#include <optional>

namespace ctxfab {

class BackendSupervisor;

class Preferences final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool conflict READ conflict NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(
        QString retentionHours READ retentionHours WRITE setRetentionHours NOTIFY changed FINAL)
    Q_PROPERTY(
        QString diagnosticLimit READ diagnosticLimit WRITE setDiagnosticLimit NOTIFY changed FINAL)
    Q_PROPERTY(QString artifactLimit READ artifactLimit WRITE setArtifactLimit NOTIFY changed FINAL)
    Q_PROPERTY(bool hasDiagnosticLimit READ hasDiagnosticLimit NOTIFY changed FINAL)
    Q_PROPERTY(bool hasArtifactLimit READ hasArtifactLimit NOTIFY changed FINAL)
    Q_PROPERTY(QString language READ language WRITE setLanguage NOTIFY changed FINAL)
    Q_PROPERTY(QString languageName READ languageName NOTIFY changed FINAL)
    Q_PROPERTY(QString languageFilter READ languageFilter WRITE setLanguageFilter NOTIFY
                   languageChoicesChanged FINAL)
    Q_PROPERTY(QStringList languageChoices READ languageChoices NOTIFY languageChoicesChanged FINAL)

  public:
    explicit Preferences(BackendSupervisor &backend);

    bool busy() const { return busy_; }
    bool ready() const { return ready_; }
    bool conflict() const { return conflict_; }
    QString feedback() const;
    QString retentionHours() const { return draft_.retentionHours; }
    QString diagnosticLimit() const { return draft_.diagnosticLimit; }
    QString artifactLimit() const { return draft_.artifactLimit; }
    QString language() const { return draft_.language; }
    QString languageName() const;
    QString languageFilter() const { return languageFilter_; }
    QStringList languageChoices() const;
    bool hasDiagnosticLimit() const;
    bool hasArtifactLimit() const;

    void setRetentionHours(const QString &value);
    void setDiagnosticLimit(const QString &value);
    void setArtifactLimit(const QString &value);
    void setLanguage(const QString &value);
    void setLanguageFilter(const QString &value);

    Q_INVOKABLE void open();
    Q_INVOKABLE void chooseLanguage(int index);
    Q_INVOKABLE void reload();
    Q_INVOKABLE void save();

  signals:
    void changed();
    void languageChoicesChanged();

  private:
    struct Draft {
        QString language;
        QString retentionHours;
        QString diagnosticLimit;
        QString artifactLimit;

        static Draft from(const openapi::Settings &settings);
        void preserveEdits(const Draft &old, const Draft &edits);
    };

    QList<openapi::AnswerLanguageOption> options() const;
    QList<openapi::AnswerLanguageOption> filteredOptions() const;
    void backendChanged();
    void accept(const openapi::HttpRequestWorker &worker, const openapi::Settings &settings,
                bool saved);
    void failed(const openapi::HttpRequestWorker &worker);
    void finish(const QString &message);
    QString validate() const;
    openapi::Settings payload() const;

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    std::optional<openapi::Settings> confirmed_;
    Draft draft_;
    QString languageFilter_;
    QString feedback_;
    bool opened_ = false;
    bool ready_ = false;
    bool busy_ = false;
    bool conflict_ = false;
};

} // namespace ctxfab
