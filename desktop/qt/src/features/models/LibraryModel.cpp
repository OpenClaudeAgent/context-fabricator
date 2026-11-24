#include "LibraryModel.h"

#include <QHash>
#include <QLocale>

#include <algorithm>

namespace ctxfab {

QString formatArtifactSize(qint64 bytes) {
    return QLocale().formattedDataSize(bytes, 2, QLocale::DataSizeSIFormat);
}

LibraryModel::LibraryModel(const openapi::ModelDownload &download,
                           const openapi::ModelStoragePlan &storage)
    : download_(download), storage_(storage), state_(stateOf(download.getState())) {}

LibraryModel::State LibraryModel::stateOf(const QString &value) {
    static const QHash<QString, State> states{{"notInstalled", State::NotInstalled},
                                              {"installing", State::Installing},
                                              {"pausing", State::Pausing},
                                              {"paused", State::Paused},
                                              {"interrupted", State::Interrupted},
                                              {"installed", State::Installed},
                                              {"failed", State::Failed}};
    return states.value(value, State::Unknown);
}

bool LibraryModel::accepts(const openapi::ModelDownload &download) {
    return download.isValid() && !download.getProfile().isEmpty() &&
           download.getDownloadedBytes() >= 0 && download.getTotalBytes() > 0 &&
           stateOf(download.getState()) != State::Unknown;
}

QString LibraryModel::label() const {
    return download_.getLabel().isEmpty() ? profile() : download_.getLabel();
}

QUrl LibraryModel::sourceUrl() const {
    const QUrl url(download_.getSource().getUrl());
    return url.scheme() == QStringLiteral("https") && !url.host().isEmpty() &&
                   url.userInfo().isEmpty()
               ? url
               : QUrl{};
}

QString LibraryModel::installingStatus(const QString &done, const QString &total) const {
    const auto stage = download_.getInstallationStage();
    if (stage == QStringLiteral("preparing")) {
        return tr("Preparing download…");
    }
    if (stage == QStringLiteral("verifying")) {
        return tr("Verifying model file…");
    }
    return tr("Downloading · %1 of %2").arg(done, total);
}

QString LibraryModel::status() const {
    const auto total = formatArtifactSize(totalBytes());
    const auto done = partialSize();
    switch (state_) {
    case State::NotInstalled:
        return tr("Not installed · %1")
            .arg(downloadedBytes() == 0 ? total : tr("%1 of %2").arg(done, total));
    case State::Installing:
        return installingStatus(done, total);
    case State::Pausing:
        return determinate() ? tr("Pausing · %1 of %2").arg(done, total) : tr("Pausing download…");
    case State::Paused:
        return tr("Paused · %1 of %2").arg(done, total);
    case State::Interrupted:
        return tr("Interrupted · %1 of %2").arg(done, total);
    case State::Installed:
        return tr("Installed · %1").arg(total);
    case State::Failed:
        return tr("Download failed · %1 of %2").arg(done, total);
    default:
        return {};
    }
}

QString LibraryModel::feedback() const {
    if (installed()) {
        return download_.getDeletionBlock();
    }
    if (state_ != State::Failed) {
        return {};
    }
    const auto error = download_.getError();
    if (error == QStringLiteral("model_integrity_failed")) {
        return tr("The download did not pass verification.");
    }
    if (error == QStringLiteral("model_storage_limit")) {
        return tr("Free disk space or raise the inference artifact limit in Settings.");
    }
    if (error == QStringLiteral("model_installation_unavailable")) {
        return tr("Model installation is currently unavailable.");
    }
    return error == QStringLiteral("model_transfer_failed")
               ? tr("Retry to resume the download.")
               : tr("Try downloading the model again.");
}

QString LibraryModel::peakSpace() const {
    if (!storage_.isValid() || installed() || transferring()) {
        return {};
    }
    const auto size = formatArtifactSize(storage_.getPeakBytes());
    return storage_.isFits() ? tr("Peak space: %1").arg(size)
                             : tr("Peak space: %1 · Insufficient available space").arg(size);
}

LibraryModel::Action LibraryModel::action() const {
    switch (state_) {
    case State::NotInstalled:
    case State::Failed:
        return Download;
    case State::Installing:
        return Pause;
    case State::Paused:
    case State::Interrupted:
        return Resume;
    case State::Installed:
        return download_.is_deletion_block_Set() ? NoAction : Remove;
    case State::Pausing:
    case State::Unknown:
        return NoAction;
    }
    return NoAction;
}

QString LibraryModel::actionLabel() const {
    switch (action()) {
    case Download:
        return state_ == State::Failed ? tr("Retry") : tr("Download");
    case Pause:
        return tr("Pause");
    case Resume:
        return tr("Resume");
    case Remove:
        return tr("Delete Model…");
    case NoAction:
        return installed() ? tr("Delete Model…") : QString{};
    }
    return {};
}

bool LibraryModel::transferring() const {
    return state_ == State::Installing || state_ == State::Pausing;
}

bool LibraryModel::determinate() const {
    const auto stage = download_.getInstallationStage();
    return downloadedBytes() > 0 && (stage.isEmpty() || stage == QStringLiteral("transferring"));
}

double LibraryModel::progress() const {
    const auto total = std::max(totalBytes(), qint64{1});
    return static_cast<double>(std::clamp(downloadedBytes(), qint64{0}, total)) /
           static_cast<double>(total);
}

bool LibraryModel::installed() const { return state_ == State::Installed; }

bool LibraryModel::inDownloads() const {
    return transferring() || canDiscard() || state_ == State::Failed;
}

bool LibraryModel::matches(const QString &query) const {
    const auto fields = {label(), profile(), kind(), source(),
                         download_.getSource().getRepository()};
    return query.isEmpty() || std::ranges::any_of(fields, [&query](const auto &field) {
               return field.contains(query, Qt::CaseInsensitive);
           });
}

bool LibraryModel::operator==(const LibraryModel &other) const {
    return download_.asJsonObject() == other.download_.asJsonObject() &&
           storage_.asJsonObject() == other.storage_.asJsonObject();
}

} // namespace ctxfab
