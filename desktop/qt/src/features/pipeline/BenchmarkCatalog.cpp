#include "BenchmarkCatalog.h"

#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QMap>
#include <QSet>

namespace ctxfab {
namespace {

QJsonDocument resource(const QString &name) {
    QFile file(QStringLiteral(":/ctxfab/benchmarks/") + name);
    return file.open(QIODevice::ReadOnly) ? QJsonDocument::fromJson(file.readAll())
                                          : QJsonDocument{};
}

BenchmarkCase question(const QJsonObject &item) {
    return {item.value("query").toString(), item.value("expected").toString(),
            item.value("source").toString()};
}

QList<BenchmarkSuite> corpora(const QJsonArray &references) {
    QMap<QString, BenchmarkSuite> groups;
    QMap<QString, QSet<QString>> seen;
    for (const auto &value : references) {
        const auto entry = value.toObject();
        const auto repo = entry.value("repository").toString();
        const auto revision = entry.value("revision").toString();
        openapi::BenchmarkReference reference;
        reference.setRepository(repo);
        reference.setRevision(revision);
        const auto id = BenchmarkCatalog::key(reference);
        auto &suite = groups[id];
        suite.id = id;
        suite.title = repo + QStringLiteral(" · ") + revision.left(8);
        suite.reference = reference;
        for (const auto &value : entry.value("questions").toArray()) {
            auto row = question(value.toObject());
            const auto normalized = row.query.simplified().toCaseFolded();
            if (!normalized.isEmpty() && !seen[id].contains(normalized)) {
                seen[id].insert(normalized);
                suite.questions.append(std::move(row));
            }
        }
    }
    return groups.values();
}

QList<BenchmarkCase> recorded(const QJsonArray &cases) {
    QList<BenchmarkCase> result;
    for (const auto &value : cases) {
        const auto item = value.toObject();
        if (!item.value("action").isObject()) {
            continue;
        }
        const auto target = item.value("action").toObject();
        openapi::BenchmarkAction action;
        action.setRepository(target.value("repository").toString());
        action.setRunId(target.value("runID").toInteger());
        action.setAttempt(target.value("attempt").toInt());
        auto row = question(item);
        row.source = action.getRepository();
        row.action = action;
        result.append(std::move(row));
    }
    return result;
}

} // namespace

BenchmarkCatalog BenchmarkCatalog::bundled() {
    auto references = resource(QStringLiteral("PipelineBenchmarks.json")).array();
    const auto full = resource(QStringLiteral("PipelineBenchmarkSuites.json")).object();
    for (const auto &reference : full.value("references").toArray()) {
        references.append(reference);
    }
    return {ctxfab::corpora(references), recorded(full.value("fixtureCases").toArray()),
            full.value("workflowQuestionTemplates").toObject()};
}

QString BenchmarkCatalog::key(const openapi::BenchmarkReference &reference) {
    return reference.getRepository() + QLatin1Char('@') + reference.getRevision();
}

QString BenchmarkCatalog::key(const openapi::BenchmarkAction &action) {
    return QStringLiteral("%1/%2/%3")
        .arg(action.getRepository())
        .arg(action.getRunId())
        .arg(action.getAttempt());
}

BenchmarkSuite
BenchmarkCatalog::downloaded(const openapi::BenchmarkActionDownload &download) const {
    const auto reference = download.getReference();
    BenchmarkSuite suite{.id = QStringLiteral("actions/") + key(reference),
                         .title = reference.getRepository() +
                                  QStringLiteral(" · GitHub Actions · ") +
                                  reference.getRevision().left(8),
                         .questions = {},
                         .ready = true};
    for (const auto &snapshot : download.getSnapshots()) {
        QMap<qint64, QString> names;
        for (const auto &workflow : snapshot.getWorkflows()) {
            names.insert(workflow.getId(), workflow.getName());
        }
        for (const auto &run : snapshot.getRuns()) {
            openapi::BenchmarkAction action;
            action.setRepository(snapshot.getRepository());
            action.setRunId(run.getId());
            action.setAttempt(run.getAttempt());
            action.setDownloaded(true);
            const auto source =
                QStringList{names.value(run.getWorkflowId(), snapshot.getRepository()),
                            snapshot.getBranch(),
                            run.getCreatedAt().toLocalTime().toString(Qt::ISODate)}
                    .join(QStringLiteral(" · "));
            const auto add = [&](const QString &field, const QString &expected) {
                if (!expected.isEmpty()) {
                    suite.questions.append(
                        {workflowPrompts.value(field).toString(), expected, source, action});
                }
            };
            add(QStringLiteral("event"), run.getEvent());
            if (run.getConclusion().isEmpty()) {
                add(QStringLiteral("status"), run.getStatus());
            } else {
                add(QStringLiteral("conclusion"), run.getConclusion());
            }
            add(QStringLiteral("branch"), run.getHeadBranch());
        }
    }
    return suite;
}

} // namespace ctxfab
