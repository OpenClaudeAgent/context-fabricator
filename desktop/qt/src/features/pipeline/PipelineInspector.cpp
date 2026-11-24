#include "PipelineInspector.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QClipboard>
#include <QGuiApplication>
#include <QJsonArray>
#include <QJsonDocument>
#include <QTimer>
#include <QUuid>
#include <algorithm>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;

QString traceId(const Worker &worker) {
    const auto headers = worker.getResponseHeaders();
    for (auto it = headers.cbegin(); it != headers.cend(); ++it) {
        if (it.key().compare(QLatin1StringView("X-Context-Trace-Id"), Qt::CaseInsensitive) == 0) {
            return it.value();
        }
    }
    return {};
}

QUrl sourceReference(const QString &source) {
    const QUrl url(source, QUrl::StrictMode);
    return url.scheme() == QStringLiteral("https") && url.host() == QStringLiteral("github.com") &&
                   url.userInfo().isEmpty() && url.port() == -1
               ? url
               : QUrl{};
}

QString sourceLabel(const QUrl &url, const QString &fallback) {
    const auto parts = url.path().split(QLatin1Char('/'), Qt::SkipEmptyParts);
    if (parts.size() > 4 && parts.at(2) == QStringLiteral("blob")) {
        return parts.sliced(4).join(QLatin1Char('/'));
    }
    return url.isEmpty() ? fallback : url.path().mid(1);
}

QString pretty(const QJsonValue &value) {
    if (value.isArray()) {
        return QString::fromUtf8(QJsonDocument(value.toArray()).toJson());
    }
    if (value.isObject()) {
        return QString::fromUtf8(QJsonDocument(value.toObject()).toJson());
    }
    return value.toString();
}

QJsonValue inspectionValue(const QJsonObject &pipeline, const QString &key,
                           const QJsonValue &fallback) {
    const auto value = pipeline.value(key);
    return value.isUndefined() ? fallback : value;
}

bool validQuery(const QString &query) { return !query.isEmpty() && query.toUcs4().size() <= 512; }

} // namespace

PipelineInspector::PipelineInspector(BackendSupervisor &backend, QSettings &settings)
    : backend_(backend), sources_(backend, settings) {
    connect(&sources_, &BenchmarkSources::suitesChanged, this, &PipelineInspector::rebuildSuites);
    connect(&sources_, &BenchmarkSources::changed, this, &PipelineInspector::changed);
    connect(&backend_, &BackendSupervisor::changed, this, [this] {
        if (busy_) {
            finish(tr("Request interrupted"));
        }
        emit changed();
    });
    rebuildSuites();
}

void PipelineInspector::open() { sources_.open(); }

void PipelineInspector::rebuildSuites() {
    const auto id = selected() ? selected()->id : QString{};
    suites_ = sources_.suites();
    if (imported_) {
        suites_.append(*imported_);
    }
    const auto match = std::ranges::find(suites_, id, &BenchmarkSuite::id);
    selectedSuite_ = match == suites_.end() ? -1 : static_cast<int>(match - suites_.begin());
    results_.clear();
    emit changed();
}

const BenchmarkSuite *PipelineInspector::selected() const {
    return selectedSuite_ >= 0 && selectedSuite_ < suites_.size() ? &suites_.at(selectedSuite_)
                                                                  : nullptr;
}

QStringList PipelineInspector::suites() const {
    QStringList labels;
    labels.reserve(suites_.size());
    for (const auto &suite : suites_) {
        labels.append(suite.title);
    }
    return labels;
}

QList<BenchmarkResultRow> PipelineInspector::rows() const {
    QList<BenchmarkResultRow> rows;
    if (!selected()) {
        return rows;
    }
    int index = 0;
    for (const auto &item : selected()->questions) {
        const auto result = results_.value(index++);
        const auto reference = sourceReference(item.source);
        rows.append({item.query, sourceLabel(reference, item.source), reference, item.expected,
                     result.result.payload().getOutcome(), result.error, result.result.text(),
                     result.result.present() || !result.error.isEmpty()});
    }
    return rows;
}

QString PipelineInspector::selectedCorpus() const {
    return selected() && selected()->reference ? selected()->id : QString{};
}

QString PipelineInspector::completionInput() const {
    return result_.payload().getEvidenceReview().getPipeline().getCompletionInput();
}

void PipelineInspector::copyCompletionInput() {
    if (auto *clipboard = QGuiApplication::clipboard()) {
        clipboard->setText(completionInput());
    }
}

bool PipelineInspector::sourceReady() const {
    return selected() && selected()->ready && backend_.session() && !sources_.busy();
}

bool PipelineInspector::hybridAvailable() const {
    return selected() && selected()->reference.has_value();
}

void PipelineInspector::selectSuite(int index) {
    if (busy_ || index < -1 || index >= suites_.size() || index == selectedSuite_) {
        return;
    }
    selectedSuite_ = index;
    results_.clear();
    feedback_.clear();
    hybridProblem_ = false;
    finished_ = 0;
    total_ = 0;
    emit changed();
}

void PipelineInspector::selectDefaultBenchmark() {
    if (selected() && !selected()->questions.isEmpty()) {
        return;
    }
    const auto ready = std::ranges::find_if(
        suites_, [](const auto &suite) { return suite.ready && !suite.questions.isEmpty(); });
    if (ready != suites_.end()) {
        selectSuite(static_cast<int>(ready - suites_.begin()));
    } else if (!suites_.isEmpty()) {
        selectSuite(0);
    }
}

void PipelineInspector::setHybrid(bool enabled) {
    if (busy_ || hybrid_ == enabled) {
        return;
    }
    hybrid_ = enabled;
    emit changed();
}

void PipelineInspector::runQuestion(int index) {
    if (sourceReady() && !busy_ && index >= 0 && index < selected()->questions.size()) {
        begin(*selected(), {{index, selected()->questions.at(index)}});
    }
}

void PipelineInspector::runAll() {
    if (!sourceReady() || busy_) {
        return;
    }
    QList<Pending> cases;
    int index = 0;
    for (const auto &item : selected()->questions) {
        cases.append({index++, item});
    }
    begin(*selected(), std::move(cases));
}

void PipelineInspector::ask(const QString &question, int selectedCase) {
    const auto query = question.trimmed();
    if (!validQuery(query) || busy_ || sources_.busy() || !backend_.session()) {
        return;
    }
    if (const auto *suite = selected()) {
        if (sourceReady()) {
            askInSuite(*suite, query, selectedCase);
        }
    } else {
        begin({}, {{-1, {query, {}, {}}}});
    }
}

void PipelineInspector::askInSuite(const BenchmarkSuite &suite, const QString &query,
                                   int selectedCase) {
    const bool hasCase = selectedCase >= 0 && selectedCase < suite.questions.size();
    if (!hasCase && !suite.reference && !suite.localScope && suite.documents.isEmpty()) {
        return;
    }
    auto item = hasCase ? suite.questions.at(selectedCase) : BenchmarkCase{query, {}, {}};
    if (item.query != query) {
        item.expected.clear();
    }
    item.query = query;
    begin(suite, {{-1, std::move(item)}});
}

void PipelineInspector::begin(BenchmarkSuite scope, QList<Pending> cases) {
    if (cases.isEmpty()) {
        return;
    }
    for (const auto &pending : cases) {
        results_.remove(pending.row);
    }
    scope_ = std::move(scope);
    queue_ = std::move(cases);
    total_ = static_cast<int>(queue_.size());
    finished_ = 0;
    indexed_ = 0;
    indexTotal_ = 0;
    busyRetries_ = 0;
    ++runEpoch_;
    busy_ = true;
    stopping_ = false;
    feedback_.clear();
    hybridProblem_ = false;
    emit changed();
    if (hybrid_ && scope_.reference) {
        indexBatch();
    } else {
        sendNext();
    }
}

void PipelineInspector::indexBatch() {
    const auto session = backend_.session();
    if (!busy_ || stopping_ || !session) {
        return;
    }
    auto *api = request_.begin(*session);
    connect(api, &Api::embedBenchmarkCorpusBatchSignalFull, this,
            [this](const Worker *, const openapi::RetrievalProgress &progress) {
                acceptIndexProgress(progress);
            });
    connect(api, &Api::embedBenchmarkCorpusBatchSignalErrorFull, this,
            &PipelineInspector::rejectIndexBatch);
    api->embedBenchmarkCorpusBatch(*scope_.reference);
}

void PipelineInspector::acceptIndexProgress(const openapi::RetrievalProgress &progress) {
    const bool incomplete = progress.getState() == QStringLiteral("incomplete");
    const bool stalled = incomplete && indexTotal_ > 0 && progress.getIndexed() <= indexed_;
    if (!progress.isValid() || progress.getTotal() < progress.getIndexed() || stalled) {
        finish(tr("Invalid response from the backend"));
        return;
    }
    indexed_ = progress.getIndexed();
    indexTotal_ = progress.getTotal();
    busyRetries_ = 0;
    emit changed();
    if (progress.getState() == QStringLiteral("ready")) {
        indexTotal_ = 0;
        sendNext();
    } else if (incomplete) {
        indexBatch();
    } else {
        hybridProblem_ = true;
        finish(tr("Hybrid requires a working embedding model. Check Models in Settings."));
    }
}

void PipelineInspector::rejectIndexBatch(const Worker *worker) {
    const auto problem = openapi::Problem(QString::fromUtf8(worker->response));
    if (problem.getCode() == QStringLiteral("embedding_busy") && busyRetries_++ < 8) {
        QTimer::singleShot(250, this, [this, epoch = runEpoch_] {
            if (epoch == runEpoch_) {
                indexBatch();
            }
        });
        return;
    }
    hybridProblem_ = QStringList{"embedding_not_configured", "embedding_runtime_unavailable",
                                 "embedding_index_disabled"}
                         .contains(problem.getCode());
    finish(LocalApiRequest::rejection(*worker, tr("Backend request failed")));
}

void PipelineInspector::sendNext() {
    const auto session = backend_.session();
    if (queue_.isEmpty() || stopping_ || !session) {
        finish(stopping_ ? tr("Benchmark stopped.") : QString{});
        return;
    }
    current_ = queue_.takeFirst();
    requestId_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
    auto *api = request_.begin(*session);
    const auto &question = current_->question;
    const bool benchmark = scope_.reference || question.action || !scope_.documents.isEmpty();
    setupAnswerSignals(api, benchmark);
    if (!benchmark) {
        auto input = scope_.localScope.value_or(openapi::Question{});
        input.setRequestId(requestId_);
        input.setQuery(question.query);
        input.setInspectPipeline(true);
        api->askQuestion(input);
        return;
    }
    sendBenchmark(*api, question);
}

void PipelineInspector::sendBenchmark(Api &api, const BenchmarkCase &question) {
    openapi::BenchmarkQuestion input;
    input.setRequestId(requestId_);
    input.setQuery(question.query);
    if (scope_.reference) {
        input.setReference(*scope_.reference);
        input.setRetrieval(hybrid_ ? QStringLiteral("hybrid") : QStringLiteral("bm25"));
    } else if (question.action) {
        input.setAction(*question.action);
    } else {
        input.setDocuments(scope_.documents);
    }
    api.askBenchmarkQuestion(input);
}

void PipelineInspector::setupAnswerSignals(Api *api, bool benchmark) {
    if (benchmark) {
        connect(api, &Api::askBenchmarkQuestionSignalFull, this, &PipelineInspector::accept);
        connect(api, &Api::askBenchmarkQuestionSignalErrorFull, this, &PipelineInspector::reject);
    } else {
        connect(api, &Api::askQuestionSignalFull, this, &PipelineInspector::accept);
        connect(api, &Api::askQuestionSignalErrorFull, this, &PipelineInspector::reject);
    }
}

void PipelineInspector::accept(const Worker *worker, const openapi::LocalAnswer &answer) {
    if (!answer.isValid()) {
        complete({{}, tr("Invalid response from the backend"), ctxfab::traceId(*worker)});
        return;
    }
    complete({AnswerResult(answer), {}, ctxfab::traceId(*worker)});
}

void PipelineInspector::reject(const Worker *worker) {
    complete({{},
              LocalApiRequest::rejection(*worker, tr("Backend request failed")),
              ctxfab::traceId(*worker)});
}

void PipelineInspector::complete(const Result &result) {
    if (current_ && !stopping_) {
        if (current_->row < 0) {
            inspect(*current_, result);
            feedback_ = result.error;
        } else {
            results_.insert(current_->row, result);
        }
        ++finished_;
        emit changed();
    }
    current_.reset();
    requestId_.clear();
    sendNext();
}

void PipelineInspector::stop() {
    if (!busy_ || stopping_) {
        return;
    }
    stopping_ = true;
    queue_.clear();
    const auto session = backend_.session();
    if (session && !requestId_.isEmpty()) {
        auto *api = cancel_.begin(*session);
        connect(api, &Api::cancelAnswerSignalFull, this,
                [this](const Worker *) { finish(tr("Benchmark stopped.")); });
        connect(api, &Api::cancelAnswerSignalErrorFull, this,
                [this](const Worker *) { finish(tr("Benchmark stopped.")); });
        api->cancelAnswer(requestId_);
        emit changed();
    } else {
        finish(tr("Benchmark stopped."));
    }
}

void PipelineInspector::finish(const QString &message) {
    request_.cancel();
    cancel_.cancel();
    queue_.clear();
    current_.reset();
    requestId_.clear();
    indexTotal_ = 0;
    busy_ = false;
    stopping_ = false;
    if (!message.isEmpty()) {
        feedback_ = message;
    }
    emit changed();
}

void PipelineInspector::inspectResult(int index) {
    if (!selected() || !results_.contains(index) || index < 0 ||
        index >= selected()->questions.size()) {
        return;
    }
    inspectedSource_ = selected()->title;
    inspect({index, selected()->questions.at(index)}, results_.value(index));
    emit changed();
}

void PipelineInspector::inspect(const Pending &question, const Result &result) {
    inspectedQuestion_ = question.question.query;
    if (question.row < 0) {
        inspectedSource_ = scope_.title.isEmpty() ? tr("All sources") : scope_.title;
    }
    result_ = result.result;
    traceId_ = result.trace;
}

QString PipelineInspector::stageText(int stage) const {
    const auto answer = result_.payload().asJsonObject();
    const auto review = answer.value("evidenceReview").toObject();
    const auto pipeline = review.value("pipeline").toObject();
    switch (stage) {
    case 0:
        return inspectedSource_;
    case 1:
        return pretty(review.value("searches"));
    case 2:
        return pretty(review.value("candidates"));
    case 3:
        return pretty(inspectionValue(pipeline, "embedding", pipeline.value("fallback")));
    case 4:
        return pretty(inspectionValue(pipeline, "fusion", review.value("candidates")));
    case 5:
        return pretty(inspectionValue(pipeline, "completionInput", answer.value("modelContext")));
    case 6:
        return pretty(pipeline.value("draft"));
    case 7:
        return pretty(QJsonObject{{"outcome", answer.value("outcome")},
                                  {"reason", review.value("reason")},
                                  {"citations", answer.value("citations")}});
    default:
        return {};
    }
}

} // namespace ctxfab
