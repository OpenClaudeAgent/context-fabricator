#pragma once

#include <QString>
#include <QUrl>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

namespace openapi {
class GitHubGraphHit;
}

class GitHubGraphOccurrence {
    Q_GADGET
    QML_VALUE_TYPE(gitHubGraphOccurrence)

    Q_PROPERTY(QString name READ name CONSTANT FINAL)
    Q_PROPERTY(QString role READ role CONSTANT FINAL)
    Q_PROPERTY(QString resolution READ resolution CONSTANT FINAL)
    Q_PROPERTY(QString sourceText READ sourceText CONSTANT FINAL)
    Q_PROPERTY(QUrl sourceUrl READ sourceUrl CONSTANT FINAL)
    Q_PROPERTY(QString targetText READ targetText CONSTANT FINAL)
    Q_PROPERTY(QUrl targetUrl READ targetUrl CONSTANT FINAL)

  public:
    GitHubGraphOccurrence() = default;
    GitHubGraphOccurrence(const openapi::GitHubGraphHit &hit, const QString &repository,
                          const QString &revision);

    [[nodiscard]] QString name() const { return name_; }
    [[nodiscard]] QString role() const { return role_; }
    [[nodiscard]] QString resolution() const { return resolution_; }
    [[nodiscard]] QString sourceText() const { return sourceText_; }
    [[nodiscard]] QUrl sourceUrl() const { return sourceUrl_; }
    [[nodiscard]] QString targetText() const { return targetText_; }
    [[nodiscard]] QUrl targetUrl() const { return targetUrl_; }

  private:
    QString name_;
    QString role_;
    QString resolution_;
    QString sourceText_;
    QUrl sourceUrl_;
    QString targetText_;
    QUrl targetUrl_;
};

} // namespace ctxfab
