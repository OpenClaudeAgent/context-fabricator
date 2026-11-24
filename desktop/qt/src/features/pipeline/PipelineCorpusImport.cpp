#include "PipelineInspector.h"

#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QRegularExpression>
#include <QStringDecoder>
#include <algorithm>

namespace ctxfab {
namespace {

constexpr qsizetype MaxFileBytes = 256 * 1024;
constexpr qsizetype MaxCorpusBytes = 512 * 1024;
constexpr qsizetype MaxSuiteBytes = 3 * 1024 * 1024;

std::optional<QByteArray> read(const QUrl &url, qsizetype limit) {
    const auto path = url.toLocalFile();
    QFile file(path);
    if (!QFileInfo(path).isFile() || !file.open(QIODevice::ReadOnly) || file.size() > limit) {
        return {};
    }
    const auto data = file.read(limit + 1);
    return data.size() <= limit ? std::optional{data} : std::nullopt;
}

bool validRelativePath(const QString &path) {
    static const QRegularExpression invalidPath(
        QStringLiteral("(^|/)(\\.{1,2}|)(/|$)|[\\x00-\\x1f\\x7f]"));
    return !path.isEmpty() && path.toUtf8().size() <= 1024 && !invalidPath.match(path).hasMatch();
}

bool validDocuments(const QList<openapi::BenchmarkDocument> &documents) {
    if (documents.isEmpty() || documents.size() > 64) {
        return false;
    }
    QSet<QString> seen;
    qsizetype total = 0;
    for (const auto &document : documents) {
        const auto path = document.getPath();
        const auto text = document.getText();
        const auto bytes = text.toUtf8().size();
        if (!validRelativePath(path) || seen.contains(path) || text.trimmed().isEmpty() ||
            text.contains(QChar::Null) || bytes > MaxFileBytes) {
            return false;
        }
        seen.insert(path);
        total += bytes;
    }
    return total <= MaxCorpusBytes;
}

QList<BenchmarkCase> readQuestions(const QJsonArray &items) {
    if (items.isEmpty() || items.size() > 128) {
        return {};
    }
    QList<BenchmarkCase> questions;
    for (const auto &item : items) {
        const auto entry = item.toObject();
        const auto title = entry.value("title").toString().trimmed();
        const auto query = entry.value("query").toString().trimmed();
        if (title.isEmpty() || title.size() > 160 || query.isEmpty() ||
            query.toUcs4().size() > 512) {
            return {};
        }
        questions.append(
            {query, entry.value("expected").toString(), entry.value("source").toString()});
    }
    return questions;
}

bool validReference(const openapi::BenchmarkReference &reference) {
    static const QRegularExpression name(QStringLiteral("\\A[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\\z"));
    static const QRegularExpression commit(QStringLiteral("\\A[0-9a-f]{40}\\z"));
    return name.match(reference.getRepository()).hasMatch() &&
           commit.match(reference.getRevision()).hasMatch();
}

} // namespace

void PipelineInspector::importFiles(const QList<QUrl> &urls) {
    if (busy_ || urls.isEmpty()) {
        return;
    }
    QList<openapi::BenchmarkDocument> documents =
        imported_ ? imported_->documents : QList<openapi::BenchmarkDocument>{};
    for (const auto &url : urls) {
        const auto data = read(url, MaxFileBytes);
        QStringDecoder decoder(QStringDecoder::Utf8);
        const QString text = data ? decoder(*data) : QString{};
        if (!data || decoder.hasError()) {
            feedback_ = tr("Select UTF-8 text files.");
            emit changed();
            return;
        }
        openapi::BenchmarkDocument item;
        item.setPath(QFileInfo(url.toLocalFile()).fileName());
        item.setText(text);
        documents.append(item);
    }
    if (!validDocuments(documents)) {
        feedback_ = tr("Use 1 to 64 text files with relative paths, up to 256 KiB per file and 512 "
                       "KiB total.");
        emit changed();
        return;
    }
    importTemporary(std::move(documents),
                    imported_ ? imported_->questions : QList<BenchmarkCase>{});
}

void PipelineInspector::importSuite(const QUrl &url) {
    if (busy_) {
        return;
    }
    const auto data = read(url, MaxSuiteBytes);
    const auto object = data ? QJsonDocument::fromJson(*data).object() : QJsonObject{};
    auto questions = readQuestions(object.value("questions").toArray());
    if (questions.isEmpty()) {
        feedback_ = tr("Use 1 to 128 questions with titles and queries of at most 512 characters.");
        emit changed();
        return;
    }
    if (object.contains("reference") && !object.contains("documents")) {
        openapi::BenchmarkReference reference;
        reference.fromJsonObject(object.value("reference").toObject());
        if (validReference(reference)) {
            const auto id = BenchmarkCatalog::key(reference);
            sources_.addCorpus({id,
                                reference.getRepository() + QStringLiteral(" · ") +
                                    reference.getRevision().left(8),
                                reference, std::move(questions)});
            const auto match = std::ranges::find(suites_, id, &BenchmarkSuite::id);
            selectSuite(static_cast<int>(match - suites_.begin()));
            feedback_.clear();
            emit changed();
            return;
        }
    }
    QList<openapi::BenchmarkDocument> documents;
    for (const auto &item : object.value("documents").toArray()) {
        openapi::BenchmarkDocument document;
        document.fromJsonObject(item.toObject());
        documents.append(std::move(document));
    }
    if (object.contains("reference") || !validDocuments(documents)) {
        feedback_ =
            tr("Use a GitHub owner/repository and a full commit ID for a benchmark reference.");
        emit changed();
        return;
    }
    importTemporary(std::move(documents), std::move(questions));
}

void PipelineInspector::importTemporary(QList<openapi::BenchmarkDocument> documents,
                                        QList<BenchmarkCase> questions) {
    imported_ = BenchmarkSuite{QStringLiteral("temporary"), tr("Temporary files"), {},
                               std::move(questions),        std::move(documents),  true};
    rebuildSuites();
    selectSuite(static_cast<int>(suites_.size() - 1));
    feedback_.clear();
    emit changed();
}

} // namespace ctxfab
