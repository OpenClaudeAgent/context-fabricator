#pragma once

#include <QObject>
#include <QStringList>
#include <QtQmlIntegration/qqmlintegration.h>

#include <memory>

class QSettings;
class QTranslator;

namespace ctxfab {

class AppLanguage final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(int selection READ selection WRITE setSelection NOTIFY changed FINAL)
    Q_PROPERTY(QStringList options READ options NOTIFY changed FINAL)

  public:
    AppLanguage(QSettings &settings, const QStringList &preferredLanguages);
    ~AppLanguage() override;

    int selection() const { return selection_; }
    QString effectiveLanguage() const { return effectiveLanguage_; }
    QStringList options() const;
    void setSelection(int selection);

  signals:
    void changed();

  private:
    bool apply(const QString &code);

    QSettings &settings_;
    QString systemLanguage_;
    QString effectiveLanguage_ = QStringLiteral("en");
    std::unique_ptr<QTranslator> translator_;
    int selection_ = 0;
};

} // namespace ctxfab
