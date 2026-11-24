#include "BenchmarkSources.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QSettings>
#include <algorithm>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;

QString corpusKey(const openapi::BenchmarkCorpus &corpus) {
    return corpus.getRepository() + QLatin1Char('@') + corpus.getRevision();
}

qsizetype runCount(const openapi::BenchmarkActionDownload &download) {
    qsizetype count = 0;
    for (const auto &snapshot : download.getSnapshots()) {
        count += snapshot.getRuns().size();
    }
    return count;
}

} // namespace

BenchmarkSources::BenchmarkSources(BackendSupervisor &backend, QSettings &settings)
    : backend_(backend), settings_(settings) {
    const auto disabled = settings_.value("pipeline/inactiveWorkflows").toStringList();
    inactive_ = QSet<QString>(disabled.cbegin(), disabled.cend());
    connect(&backend_, &BackendSupervisor::changed, this, [this] {
        forgetSession();
        if (opened_) {
            refresh();
        }
    });
}

QList<BenchmarkCorpusRow> BenchmarkSources::corpora() const {
    QList<BenchmarkCorpusRow> rows;
    for (const auto &suite : catalog_.corpora) {
        const auto saved = saved_.value(suite.id);
        rows.append({suite.id, suite.reference->getRepository(),
                     suite.reference->getRevision().left(12), saved_.contains(suite.id),
                     saved.isActive(), downloads_.contains(suite.id),
                     runCount(downloads_.value(suite.id))});
    }
    return rows;
}

QString BenchmarkSources::workflowKey(const openapi::BenchmarkAction &action) const {
    const auto origin =
        action.isDownloaded() ? QStringLiteral("public/") : QStringLiteral("recorded/");
    return origin + BenchmarkCatalog::key(action);
}

bool BenchmarkSources::workflowActive(const openapi::BenchmarkAction &action) const {
    return !inactive_.contains(workflowKey(action));
}

QList<BenchmarkWorkflowRow>
BenchmarkSources::workflowRows(const QList<openapi::GitHubActions> &snapshots,
                               bool downloaded) const {
    QList<BenchmarkWorkflowRow> rows;
    for (const auto &snapshot : snapshots) {
        QMap<qint64, QString> names;
        for (const auto &workflow : snapshot.getWorkflows()) {
            names.insert(workflow.getId(), workflow.getName());
        }
        for (const auto &run : snapshot.getRuns()) {
            openapi::BenchmarkAction action;
            action.setRepository(snapshot.getRepository());
            action.setRunId(run.getId());
            action.setAttempt(run.getAttempt());
            action.setDownloaded(downloaded);
            rows.append({workflowKey(action),
                         snapshot.getRepository() + QStringLiteral(" · ") +
                             names.value(run.getWorkflowId(), snapshot.getBranch()),
                         snapshot.getBranch() + QStringLiteral(" · ") +
                             run.getCreatedAt().toLocalTime().toString(Qt::ISODate),
                         workflowActive(action), downloaded});
        }
    }
    return rows;
}

QList<BenchmarkWorkflowRow> BenchmarkSources::workflows() const {
    auto rows = workflowRows(captures_, false);
    for (const auto &download : downloads_) {
        rows.append(workflowRows(download.getSnapshots(), true));
    }
    return rows;
}

bool BenchmarkSources::canActivateCorpora() const {
    return !busy_ && std::ranges::any_of(catalog_.corpora, [this](const auto &suite) {
        return !active(*suite.reference);
    });
}

bool BenchmarkSources::active(const openapi::BenchmarkReference &reference) const {
    return saved_.value(BenchmarkCatalog::key(reference)).isActive();
}

BenchmarkSuite BenchmarkSources::recordedSuite() const {
    BenchmarkSuite recorded{.id = QStringLiteral("recorded"),
                            .title = tr("Recorded Benchmark Attempts"),
                            .questions = {}};
    QSet<QString> observed;
    for (const auto &row : workflowRows(captures_, false)) {
        observed.insert(row.key);
    }
    for (const auto &row : catalog_.captures) {
        if (!loadedCaptures_ ||
            (observed.contains(workflowKey(*row.action)) && workflowActive(*row.action))) {
            recorded.questions.append(row);
        }
    }
    recorded.ready = loadedCaptures_ && !recorded.questions.isEmpty();
    return recorded;
}

QList<BenchmarkSuite> BenchmarkSources::suites() const {
    auto result = catalog_.corpora;
    for (auto &suite : result) {
        suite.ready = active(*suite.reference);
    }
    auto recorded = recordedSuite();
    if (!recorded.questions.isEmpty()) {
        result.append(std::move(recorded));
    }
    for (const auto &download : downloads_) {
        auto suite = catalog_.downloaded(download);
        suite.questions.removeIf([this](const auto &row) { return !workflowActive(*row.action); });
        if (!suite.questions.isEmpty()) {
            result.append(std::move(suite));
        }
    }
    result.append(connectedSuites());
    return result;
}

void BenchmarkSources::addCorpus(BenchmarkSuite suite) {
    const auto found = std::ranges::find(catalog_.corpora, suite.id, &BenchmarkSuite::id);
    if (found == catalog_.corpora.end()) {
        catalog_.corpora.append(std::move(suite));
    } else {
        for (const auto &question : suite.questions) {
            if (std::ranges::find(found->questions, question.query, &BenchmarkCase::query) ==
                found->questions.end()) {
                found->questions.append(question);
            }
        }
    }
    emit changed();
    emit suitesChanged();
}

const BenchmarkSuite *BenchmarkSources::corpus(const QString &id) const {
    const auto found = std::ranges::find(catalog_.corpora, id, &BenchmarkSuite::id);
    return found == catalog_.corpora.end() ? nullptr : &*found;
}

Api *BenchmarkSources::begin() {
    const auto session = backend_.session();
    return session ? request_.begin(*session) : nullptr;
}

void BenchmarkSources::forgetSession() {
    request_.cancel();
    busy_ = false;
    loadedCaptures_ = false;
    loadingOverview_ = false;
    connectedLoaded_ = false;
    pendingCorpus_.clear();
    saved_.clear();
    downloads_.clear();
    captures_.clear();
    repositories_.clear();
    connected_.clear();
    pendingConnected_.clear();
    downloadQueue_.clear();
    downloadFailures_.clear();
    activationQueue_.clear();
    activationFailures_.clear();
    emit changed();
    emit suitesChanged();
}

void BenchmarkSources::open() {
    opened_ = true;
    if (!busy_) {
        refresh();
    }
}

void BenchmarkSources::refresh() {
    auto *api = begin();
    if (!api) {
        finish(tr("Backend request failed"));
        return;
    }
    busy_ = true;
    loadingOverview_ = true;
    feedback_.clear();
    emit changed();
    connect(api, &Api::listBenchmarkCorporaSignalFull, this,
            [this](const Worker *, const QList<openapi::BenchmarkCorpus> &list) {
                saved_.clear();
                for (const auto &entry : list) {
                    const auto id = corpusKey(entry);
                    saved_.insert(id, entry);
                    if (!corpus(id)) {
                        openapi::BenchmarkReference reference;
                        reference.setRepository(entry.getRepository());
                        reference.setRevision(entry.getRevision());
                        catalog_.corpora.append({.id = id,
                                                 .title = entry.getRepository() +
                                                          QStringLiteral(" · ") +
                                                          entry.getRevision().left(8),
                                                 .reference = reference,
                                                 .questions = {}});
                    }
                }
                refreshDownloads();
            });
    connect(api, &Api::listBenchmarkCorporaSignalErrorFull, this, &BenchmarkSources::failure);
    api->listBenchmarkCorpora();
}

void BenchmarkSources::refreshDownloads() {
    auto *api = begin();
    if (!api) {
        finish(tr("Backend request failed"));
        return;
    }
    connect(api, &Api::listBenchmarkActionDownloadsSignalFull, this,
            [this](const Worker *, const QList<openapi::BenchmarkActionDownload> &list) {
                downloads_.clear();
                for (const auto &entry : list) {
                    downloads_.insert(BenchmarkCatalog::key(entry.getReference()), entry);
                }
                if (settings_.value("pipeline/recordedCaptures", false).toBool()) {
                    fetchCaptures();
                } else {
                    finish();
                }
            });
    connect(api, &Api::listBenchmarkActionDownloadsSignalErrorFull, this,
            &BenchmarkSources::failure);
    api->listBenchmarkActionDownloads();
}

void BenchmarkSources::setActive(const QString &id, bool enabled) {
    const auto *source = corpus(id);
    if (busy_ || !source || saved_.value(id).isActive() == enabled) {
        return;
    }
    auto *api = begin();
    if (!api) {
        finish(tr("Backend request failed"));
        return;
    }
    busy_ = true;
    pendingCorpus_ = id;
    feedback_.clear();
    emit changed();
    if (enabled) {
        connect(api, &Api::loadBenchmarkCorpusSignalFull, this,
                [this, id](const Worker *, openapi::BenchmarkCorpus entry) {
                    entry.setActive(true);
                    saved_.insert(id, entry);
                    finish();
                });
        connect(api, &Api::loadBenchmarkCorpusSignalErrorFull, this, &BenchmarkSources::failure);
        api->loadBenchmarkCorpus(*source->reference);
    } else {
        connect(api, &Api::unloadBenchmarkCorpusSignalFull, this, [this, id](const Worker *) {
            saved_[id].setActive(false);
            finish();
        });
        connect(api, &Api::unloadBenchmarkCorpusSignalErrorFull, this, &BenchmarkSources::failure);
        api->unloadBenchmarkCorpus(*source->reference);
    }
}

void BenchmarkSources::activateAllCorpora() {
    if (busy_) {
        return;
    }
    for (const auto &suite : catalog_.corpora) {
        if (!active(*suite.reference)) {
            activationQueue_.enqueue(*suite.reference);
        }
    }
    activateNext();
}

void BenchmarkSources::activateNext() {
    if (activationQueue_.isEmpty()) {
        const auto message = activationFailures_.join(QLatin1Char('\n'));
        activationFailures_.clear();
        finish(message);
        return;
    }
    auto *api = begin();
    if (!api) {
        activationQueue_.clear();
        finish(tr("Backend request failed"));
        return;
    }
    busy_ = true;
    const auto target = activationQueue_.dequeue();
    pendingCorpus_ = BenchmarkCatalog::key(target);
    emit changed();
    connect(api, &Api::loadBenchmarkCorpusSignalFull, this,
            [this](const Worker *, openapi::BenchmarkCorpus entry) {
                entry.setActive(true);
                saved_.insert(corpusKey(entry), entry);
                activateNext();
            });
    connect(api, &Api::loadBenchmarkCorpusSignalErrorFull, this,
            [this, target](const Worker *worker) {
                activationFailures_.append(
                    target.getRepository() + QStringLiteral(" · ") +
                    LocalApiRequest::rejection(*worker, tr("Backend request failed")));
                activateNext();
            });
    api->loadBenchmarkCorpus(target);
}

void BenchmarkSources::downloadActions(const QString &id) {
    if (const auto *source = corpus(id); source && !busy_) {
        downloadQueue_.enqueue(*source->reference);
        downloadNext();
    }
}

void BenchmarkSources::downloadAllActions() {
    if (busy_) {
        return;
    }
    for (const auto &suite : catalog_.corpora) {
        downloadQueue_.enqueue(*suite.reference);
    }
    downloadNext();
}

void BenchmarkSources::downloadNext() {
    if (downloadQueue_.isEmpty()) {
        const auto message = downloadFailures_.join(QLatin1Char('\n'));
        downloadFailures_.clear();
        finish(message);
        return;
    }
    auto *api = begin();
    if (!api) {
        downloadQueue_.clear();
        finish(tr("Backend request failed"));
        return;
    }
    const auto target = downloadQueue_.dequeue();
    pendingCorpus_ = BenchmarkCatalog::key(target);
    busy_ = true;
    feedback_.clear();
    emit changed();
    connect(api, &Api::downloadBenchmarkActionsSignalFull, this,
            [this](const Worker *, const openapi::BenchmarkActionDownload &result) {
                downloads_.insert(BenchmarkCatalog::key(result.getReference()), result);
                downloadNext();
            });
    connect(api, &Api::downloadBenchmarkActionsSignalErrorFull, this,
            [this, target](const Worker *worker) {
                downloadFailures_.append(
                    target.getRepository() + QStringLiteral(" · ") +
                    LocalApiRequest::rejection(*worker, tr("Backend request failed")));
                downloadNext();
            });
    api->downloadBenchmarkActions(target);
}

void BenchmarkSources::loadRecordedCaptures() {
    if (!busy_) {
        fetchCaptures();
    }
}

void BenchmarkSources::fetchCaptures() {
    auto *api = begin();
    if (!api) {
        finish(tr("Backend request failed"));
        return;
    }
    busy_ = true;
    feedback_.clear();
    emit changed();
    connect(api, &Api::listRecordedBenchmarkActionsSignalFull, this,
            [this](const Worker *, const QList<openapi::GitHubActions> &snapshots) {
                captures_ = snapshots;
                loadedCaptures_ = true;
                settings_.setValue("pipeline/recordedCaptures", true);
                finish();
            });
    connect(api, &Api::listRecordedBenchmarkActionsSignalErrorFull, this,
            &BenchmarkSources::failure);
    api->listRecordedBenchmarkActions();
}

void BenchmarkSources::setWorkflowActive(const QString &id, bool enabled) {
    if (busy_) {
        return;
    }
    if (enabled) {
        inactive_.remove(id);
    } else {
        inactive_.insert(id);
    }
    settings_.setValue("pipeline/inactiveWorkflows", QStringList(inactive_.values()));
    emit changed();
    emit suitesChanged();
}

void BenchmarkSources::failure(const Worker *worker) {
    finish(LocalApiRequest::rejection(*worker, tr("Backend request failed")));
}

void BenchmarkSources::finish(const QString &message) {
    request_.cancel();
    busy_ = false;
    feedback_ = message;
    loadingOverview_ = false;
    pendingCorpus_.clear();
    emit changed();
    emit suitesChanged();
}

} // namespace ctxfab
