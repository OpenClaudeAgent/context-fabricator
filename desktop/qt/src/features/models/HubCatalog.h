#pragma once

#include "HubModelKind.h"
#include "HubModelRepository.h"
#include "HubModelVariant.h"
#include "LocalApiRequest.h"

#include <QList>
#include <QUrl>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class BackendSupervisor;

class HubCatalogRow {
    Q_GADGET
    QML_VALUE_TYPE(hubCatalogRow)
    Q_PROPERTY(QString label READ label CONSTANT FINAL)
    Q_PROPERTY(QString size READ size CONSTANT FINAL)
    Q_PROPERTY(bool selectable READ selectable CONSTANT FINAL)

  public:
    HubCatalogRow() = default;
    explicit HubCatalogRow(QString label, QString size = {}, bool selectable = true);

    QString label() const { return label_; }
    QString size() const { return size_; }
    bool selectable() const { return selectable_; }

  private:
    QString label_;
    QString size_;
    bool selectable_ = false;
};

class HubCatalog final : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(HubCatalog)
    QML_UNCREATABLE("Owned by the desktop application")
    Q_PROPERTY(bool active READ active NOTIFY changed FINAL)
    Q_PROPERTY(Kind kind READ kind NOTIFY changed FINAL)
    Q_PROPERTY(QString query READ query WRITE setQuery NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool addingDownload READ addingDownload NOTIFY changed FINAL)
    Q_PROPERTY(QString busyText READ busyText NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(bool hasResults READ hasResults NOTIFY changed FINAL)
    Q_PROPERTY(bool variantsReady READ variantsReady NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::HubCatalogRow> repositories READ repositories NOTIFY
                   repositoriesChanged FINAL)
    Q_PROPERTY(QList<ctxfab::HubCatalogRow> variants READ variants NOTIFY variantsChanged FINAL)
    Q_PROPERTY(QString repositoryName READ repositoryName NOTIFY changed FINAL)
    Q_PROPERTY(QUrl repositoryUrl READ repositoryUrl NOTIFY changed FINAL)
    Q_PROPERTY(int selectedVariantIndex READ selectedVariantIndex NOTIFY changed FINAL)
    Q_PROPERTY(bool canDownload READ canDownload NOTIFY changed FINAL)

  public:
    enum Kind { Generation, Embedding };
    Q_ENUM(Kind)

    explicit HubCatalog(BackendSupervisor &backend);

    bool active() const { return active_; }
    Kind kind() const { return kind_; }
    QString query() const { return query_; }
    bool busy() const { return operation_ != Operation::None; }
    bool addingDownload() const { return operation_ == Operation::Download; }
    QString busyText() const;
    QString feedback() const { return feedback_; }
    bool hasResults() const { return repositories_.has_value(); }
    bool variantsReady() const { return variants_.has_value(); }
    QList<HubCatalogRow> repositories() const;
    QList<HubCatalogRow> variants() const;
    QString repositoryName() const;
    QUrl repositoryUrl() const;
    int selectedVariantIndex() const { return selectedVariantIndex_; }
    bool canDownload() const { return active() && !busy() && selectedVariantIndex_ >= 0; }

    void setQuery(const QString &query);
    Q_INVOKABLE void open();
    Q_INVOKABLE void close();
    Q_INVOKABLE void selectKind(Kind kind);
    Q_INVOKABLE void search();
    Q_INVOKABLE void openRepository(int index);
    Q_INVOKABLE void back();
    Q_INVOKABLE void selectVariant(int index);
    Q_INVOKABLE void download();

  signals:
    void changed();
    void repositoriesChanged();
    void variantsChanged();
    void downloadAccepted(const QString &profile);

  private:
    enum class Operation { None, Search, Variants, Download };

    openapi::HubModelKind apiKind() const;
    bool variantSelectable(const openapi::HubModelVariant &variant) const;
    void reset(bool active = false, Kind kind = Generation);
    void clearDetail();
    void fail(const QString &message);
    void acceptRepositories(const openapi::HttpRequestWorker &worker,
                            QList<openapi::HubModelRepository> rows);
    void acceptVariants(const openapi::HttpRequestWorker &worker,
                        QList<openapi::HubModelVariant> rows);

    BackendSupervisor &backend_;
    LocalApiRequest request_{120'000};
    bool active_ = false;
    Kind kind_ = Generation;
    Operation operation_ = Operation::None;
    QString query_;
    QString feedback_;
    std::optional<QList<openapi::HubModelRepository>> repositories_;
    std::optional<QList<openapi::HubModelVariant>> variants_;
    int repositoryIndex_ = -1;
    int selectedVariantIndex_ = -1;
};

} // namespace ctxfab
