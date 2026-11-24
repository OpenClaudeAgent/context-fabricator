#include "HubCatalog.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"
#include "LibraryModel.h"

#include <QJsonDocument>
#include <QRegularExpression>

#include <algorithm>
#include <utility>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
using ApiKind = openapi::HubModelKind::eHubModelKind;

bool validRepository(const openapi::HubModelRepository &repository, ApiKind kind) {
    static const QRegularExpression revisionPattern(QStringLiteral("^[0-9a-f]{40}\\z"));
    const QUrl url(repository.getUrl(), QUrl::StrictMode);
    return repository.isValid() && !repository.getRepository().isEmpty() &&
           revisionPattern.match(repository.getRevision()).hasMatch() && url.isValid() &&
           url.scheme() == QStringLiteral("https") &&
           url.host() == QStringLiteral("huggingface.co") && url.userInfo().isEmpty() &&
           (!repository.is_kind_Set() ||
            (repository.getKind().isValid() && repository.getKind().getValue() == kind));
}

bool validVariant(const openapi::HubModelVariant &variant) {
    static const QRegularExpression digestPattern(QStringLiteral("^[0-9a-f]{64}\\z"));
    return variant.isValid() && !variant.getFile().isEmpty() && variant.getBytes() > 0 &&
           variant.getBytes() <= 5'000'000'000LL &&
           (!variant.is_sha256_Set() || digestPattern.match(variant.getSha256()).hasMatch());
}

} // namespace

HubCatalogRow::HubCatalogRow(QString label, QString size, bool selectable)
    : label_(std::move(label)), size_(std::move(size)), selectable_(selectable) {}

HubCatalog::HubCatalog(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, [this] { reset(); });
}

QString HubCatalog::busyText() const {
    switch (operation_) {
    case Operation::Search:
        return tr("Searching Hugging Face…");
    case Operation::Variants:
        return tr("Checking compatible files…");
    case Operation::Download:
        return tr("Preparing verified download…");
    case Operation::None:
        return {};
    }
    Q_UNREACHABLE();
}

QList<HubCatalogRow> HubCatalog::repositories() const {
    QList<HubCatalogRow> rows;
    if (repositories_) {
        rows.reserve(repositories_->size());
        for (const auto &repository : *repositories_) {
            rows.emplaceBack(repository.getRepository());
        }
    }
    return rows;
}

QList<HubCatalogRow> HubCatalog::variants() const {
    QList<HubCatalogRow> rows;
    if (variants_) {
        rows.reserve(variants_->size());
        for (const auto &variant : *variants_) {
            rows.emplaceBack(variant.getFile(), formatArtifactSize(variant.getBytes()),
                             variantSelectable(variant));
        }
    }
    return rows;
}

QString HubCatalog::repositoryName() const {
    return repositoryIndex_ < 0 ? QString{} : repositories_->at(repositoryIndex_).getRepository();
}

QUrl HubCatalog::repositoryUrl() const {
    return repositoryIndex_ < 0 ? QUrl{} : QUrl(repositories_->at(repositoryIndex_).getUrl());
}

void HubCatalog::setQuery(const QString &query) {
    if (active_ && query_ != query) {
        query_ = query;
        emit changed();
    }
}

void HubCatalog::open() {
    if (!active_ && backend_.session()) {
        reset(true);
    }
}

void HubCatalog::close() {
    if (active_ && !addingDownload()) {
        reset();
    }
}

void HubCatalog::selectKind(Kind kind) {
    if (active_ && !addingDownload() && kind != kind_) {
        reset(true, kind);
    }
}

openapi::HubModelKind HubCatalog::apiKind() const {
    openapi::HubModelKind result;
    result.setValue(kind_ == Embedding ? ApiKind::EMBEDDING : ApiKind::GENERATION);
    return result;
}

void HubCatalog::search() {
    const auto session = backend_.session();
    const auto term = query_.trimmed();
    if (!active_ || !session || busy() || repositoryIndex_ >= 0 || term.isEmpty()) {
        return;
    }
    repositories_.reset();
    feedback_.clear();
    operation_ = Operation::Search;
    auto *api = request_.begin(*session);
    connect(api, &Api::searchModelCatalogSignalFull, this,
            [this](const Worker *worker, QList<openapi::HubModelRepository> rows) {
                acceptRepositories(*worker, std::move(rows));
            });
    connect(api, &Api::searchModelCatalogSignalErrorFull, this, [this](const Worker *worker) {
        fail(LocalApiRequest::rejection(*worker, tr("Couldn’t Load Models")));
    });
    api->searchModelCatalog(term, apiKind());
    emit repositoriesChanged();
    emit changed();
}

void HubCatalog::acceptRepositories(const Worker &worker, QList<openapi::HubModelRepository> rows) {
    const auto role = apiKind().getValue();
    if (worker.getHttpResponseCode() != 200 || rows.size() > 20 ||
        !QJsonDocument::fromJson(worker.response).isArray() ||
        !std::ranges::all_of(rows,
                             [role](const auto &row) { return validRepository(row, role); })) {
        fail(tr("Couldn’t Load Models"));
        return;
    }
    operation_ = Operation::None;
    repositories_ = std::move(rows);
    emit repositoriesChanged();
    emit changed();
}

void HubCatalog::openRepository(int index) {
    const auto session = backend_.session();
    if (!active_ || !session || busy() || repositoryIndex_ >= 0 || !repositories_ || index < 0 ||
        index >= repositories_->size()) {
        return;
    }
    repositoryIndex_ = index;
    const auto &repository = repositories_->at(index);
    feedback_.clear();
    operation_ = Operation::Variants;
    auto *api = request_.begin(*session);
    connect(api, &Api::readModelCatalogVariantsSignalFull, this,
            [this](const Worker *worker, QList<openapi::HubModelVariant> rows) {
                acceptVariants(*worker, std::move(rows));
            });
    connect(api, &Api::readModelCatalogVariantsSignalErrorFull, this, [this](const Worker *worker) {
        fail(LocalApiRequest::rejection(*worker, tr("Couldn’t Load Models")));
    });
    api->readModelCatalogVariants(repository.getRepository(), repository.getRevision(), apiKind());
    emit changed();
}

void HubCatalog::acceptVariants(const Worker &worker, QList<openapi::HubModelVariant> rows) {
    if (worker.getHttpResponseCode() != 200 || rows.size() > 12 ||
        !QJsonDocument::fromJson(worker.response).isArray() ||
        !std::ranges::all_of(rows, validVariant)) {
        fail(tr("Couldn’t Load Models"));
        return;
    }
    operation_ = Operation::None;
    variants_ = std::move(rows);
    emit variantsChanged();
    emit changed();
}

bool HubCatalog::variantSelectable(const openapi::HubModelVariant &variant) const {
    return kind_ == Generation || variant.is_sha256_Set();
}

void HubCatalog::selectVariant(int index) {
    if (!busy() && variants_ && index >= 0 && index < variants_->size() &&
        variantSelectable(variants_->at(index))) {
        selectedVariantIndex_ = index;
        emit changed();
    }
}

void HubCatalog::download() {
    const auto session = backend_.session();
    if (!session || !canDownload()) {
        return;
    }
    const auto &repository = repositories_->at(repositoryIndex_);
    const auto &variant = variants_->at(selectedVariantIndex_);
    openapi::HubModelDownloadRequest selection;
    selection.setRepository(repository.getRepository());
    selection.setRevision(repository.getRevision());
    selection.setKind(apiKind());
    selection.setFile(variant.getFile());
    if (variant.is_sha256_Set()) {
        selection.setSha256(variant.getSha256());
    }
    feedback_.clear();
    operation_ = Operation::Download;
    auto *api = request_.begin(*session);
    connect(api, &Api::requestHubModelDownloadSignalFull, this,
            [this](const Worker *worker, const openapi::ModelDownloadRequest &download) {
                const auto profile = download.getProfile();
                if (worker->getHttpResponseCode() != 202 || !download.isValid() ||
                    profile.isEmpty() || profile.size() > 128) {
                    fail(tr("Download failed"));
                    return;
                }
                reset();
                emit downloadAccepted(profile);
            });
    connect(api, &Api::requestHubModelDownloadSignalErrorFull, this, [this](const Worker *worker) {
        const auto message = LocalApiRequest::rejection(*worker, tr("Download failed"));
        fail(worker->getHttpResponseCode() == 409
                 ? tr("Search again and review current files. %1").arg(message)
                 : message);
    });
    api->requestHubModelDownload(selection);
    emit changed();
}

void HubCatalog::back() {
    if (repositoryIndex_ >= 0 && !addingDownload()) {
        clearDetail();
        emit variantsChanged();
        emit changed();
    }
}

void HubCatalog::clearDetail() {
    request_.cancel();
    operation_ = Operation::None;
    repositoryIndex_ = -1;
    selectedVariantIndex_ = -1;
    variants_.reset();
    feedback_.clear();
}

void HubCatalog::reset(bool active, Kind kind) {
    clearDetail();
    active_ = active;
    kind_ = kind;
    if (!active_) {
        query_.clear();
    }
    repositories_.reset();
    emit repositoriesChanged();
    emit variantsChanged();
    emit changed();
}

void HubCatalog::fail(const QString &message) {
    operation_ = Operation::None;
    feedback_ = message;
    emit changed();
}

} // namespace ctxfab
