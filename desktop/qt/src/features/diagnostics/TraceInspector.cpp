#include "TraceInspector.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QClipboard>
#include <QGuiApplication>
#include <QRegularExpression>
#include <QSet>

#include <algorithm>
#include <iterator>
#include <ranges>
#include <tuple>
#include <utility>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
using Page = openapi::DiagnosticTracePage;
using Sample = openapi::SignalSample;

bool isSpanId(const QString &value) {
    static const QRegularExpression pattern(QStringLiteral("\\A[a-f0-9]{16}\\z"));
    return pattern.match(value).hasMatch();
}

auto sortOrder(const Sample &sample) {
    constexpr qint64 MicrosPerSecond = 1000000;
    const auto micros = sample.getStartedAtUnixMicros();
    return sample.is_started_at_unix_micros_Set()
               ? std::tuple{micros / MicrosPerSecond, micros % MicrosPerSecond, sample.getId()}
               : std::tuple{sample.getTimestamp(), qint64{0}, sample.getId()};
}

using Children = QHash<QString, QList<qsizetype>>;

void appendTree(const QList<Sample> &ordered, const Children &children, qsizetype root,
                bool parentMissing, QSet<qsizetype> &visited, QList<TraceRecord> &outline) {
    struct Node {
        qsizetype index;
        int depth;
    };
    QList<Node> pending{{root, 0}};
    while (!pending.isEmpty()) {
        const auto node = pending.takeLast();
        if (visited.contains(node.index)) {
            continue;
        }
        visited.insert(node.index);
        const auto &record = ordered.at(node.index);
        outline.append(TraceRecord(record, node.depth, node.depth == 0 && parentMissing));
        const auto branches = children.value(record.getSpanId());
        for (const auto index : branches | std::views::reverse) {
            pending.append({index, node.depth + 1});
        }
    }
}

QList<TraceRecord> makeOutline(QList<Sample> ordered) {
    std::ranges::sort(ordered, std::ranges::less{}, sortOrder);
    QSet<QString> known;
    Children children;
    for (qsizetype index = 0; index < ordered.size(); ++index) {
        const auto &sample = ordered.at(index);
        known.insert(sample.getSpanId());
        if (sample.is_parent_span_id_Set()) {
            children[sample.getParentSpanId()].append(index);
        }
    }
    QList<TraceRecord> outline;
    QSet<qsizetype> visited;
    for (qsizetype index = 0; index < ordered.size(); ++index) {
        const auto parent = ordered.at(index).getParentSpanId();
        if (parent.isEmpty() || !known.contains(parent)) {
            appendTree(ordered, children, index, !parent.isEmpty(), visited, outline);
        }
    }
    for (qsizetype index = 0; index < ordered.size(); ++index) {
        appendTree(ordered, children, index, true, visited, outline);
    }
    return outline;
}

bool validPage(const Page &page, const QString &trace, qint64 before) {
    if (!page.isValid()) {
        return false;
    }
    const auto next = page.getNextBeforeId();
    if (page.is_next_before_id_Set() && (next <= 0 || (before > 0 && next >= before))) {
        return false;
    }
    const auto inScope = [&trace, before](const Sample &record) {
        return record.getId() > 0 && (before == 0 || record.getId() < before) &&
               record.getTraceId() == trace;
    };
    return std::ranges::all_of(page.getSpans(), inScope) &&
           std::ranges::all_of(page.getLogs(), inScope);
}

} // namespace

TraceInspector::TraceInspector(BackendSupervisor &backend) : backend_(backend) {
    retry_.setSingleShot(true);
    connect(&retry_, &QTimer::timeout, this, [this] { load(0, false); });
    connect(&backend_, &BackendSupervisor::changed, this, &TraceInspector::close);
}

QString TraceInspector::feedback() const { return tr(qUtf8Printable(feedback_)); }

bool TraceInspector::isAnswerTrace() const {
    return std::ranges::any_of(spanSamples_,
                               [](const Sample &span) { return span.getName() == "api_answer"; });
}

bool TraceInspector::recordedAbstention() const {
    return std::ranges::any_of(spanSamples_, [](const Sample &span) {
        return span.getSpanOutcome().getValue() ==
               openapi::DiagnosticSpanOutcome::eDiagnosticSpanOutcome::INSUFFICIENTEVIDENCE;
    });
}

QList<TraceDetailField> TraceInspector::recordedRetrieval() const {
    const auto retrieval = std::ranges::find_if(
        spanSamples_, [](const Sample &span) { return span.getName() == "answer_retrieval"; });
    return retrieval == spanSamples_.end() ? QList<TraceDetailField>{}
                                           : RecordedAnswerDetails(*retrieval).retrieval();
}

QString TraceInspector::abstentionDetail() const {
    const bool noEvidence = std::ranges::any_of(spanSamples_, [](const Sample &span) {
        return span.getName() == "answer_retrieval" &&
               std::ranges::any_of(span.getEvents(), [](const auto &event) {
                   return event.getName() == "No evidence found";
               });
    });
    return noEvidence ? tr("The recorded retrieval step found no usable evidence.")
                      : tr("This request ended without a verified answer.");
}

QStringList TraceInspector::spanTitles() const {
    QStringList titles;
    for (const auto &item : outline_) {
        titles.append(item.title());
    }
    return titles;
}

int TraceInspector::selectedIndex() const {
    if (outline_.isEmpty()) {
        return -1;
    }
    const auto ownSelection = std::ranges::find(outline_, selectedId_, &TraceRecord::id);
    if (ownSelection != outline_.end()) {
        return static_cast<int>(std::distance(outline_.begin(), ownSelection));
    }
    if (destination_.span.isEmpty()) {
        return 0;
    }
    const auto initial = std::ranges::find(outline_, destination_.span, &TraceRecord::spanId);
    return initial != outline_.end() ? static_cast<int>(std::distance(outline_.begin(), initial))
                                     : 0;
}

QList<TraceRecord> TraceInspector::spanLogs() const {
    QList<TraceRecord> result;
    const auto span = selected().spanId();
    if (!span.isEmpty()) {
        std::ranges::copy_if(logs_, std::back_inserter(result), [&span](const TraceRecord &record) {
            return record.spanId() == span;
        });
    }
    return result;
}

void TraceInspector::open(const QString &trace, const QString &span) {
    if (TraceRecord::isValidTraceId(trace) && (span.isEmpty() || isSpanId(span))) {
        ancestry_.clear();
        openDestination({trace, span});
    }
}

void TraceInspector::openDestination(Destination destination) {
    resetRecords();
    destination_ = std::move(destination);
    emit destinationChanged();
    load(0, true);
}

void TraceInspector::resetRecords() {
    retry_.stop();
    request_.cancel();
    spanSamples_.clear();
    outline_.clear();
    logs_.clear();
    feedback_.clear();
    selectedId_ = 0;
    beforeId_ = 0;
    busy_ = false;
    emit recordsChanged();
    emit selectionChanged();
}

void TraceInspector::close() {
    if (visible()) {
        resetRecords();
        destination_ = {};
        ancestry_.clear();
        emit destinationChanged();
        emit changed();
    }
}

void TraceInspector::reload() {
    if (visible() && !busy_) {
        resetRecords();
        load(0, true);
    }
}

void TraceInspector::loadMore() {
    if (hasMore() && !busy_) {
        load(beforeId_, false);
    }
}

void TraceInspector::selectSpan(int index) {
    if (index >= 0 && index < outline_.size() && outline_.at(index).id() != selectedId_) {
        selectedId_ = outline_.at(index).id();
        emit selectionChanged();
    }
}

void TraceInspector::followWorkTrace() {
    const auto current = selected();
    const auto next = current.workTraceId();
    if (TraceRecord::isValidTraceId(next) && next != destination_.trace) {
        ancestry_.append(Destination{destination_.trace, current.spanId()});
        openDestination({next, {}});
    }
}

void TraceInspector::goBack() {
    if (canGoBack()) {
        openDestination(ancestry_.takeLast());
    }
}

void TraceInspector::copyTraceId() {
    if (visible()) {
        QGuiApplication::clipboard()->setText(traceId());
    }
}

void TraceInspector::load(qint64 before, bool canRetry) {
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    auto *api = request_.begin(*session);
    busy_ = true;
    feedback_.clear();
    connect(api, &Api::readTraceSignalFull, this,
            [this, before, canRetry](const Worker *worker, const Page &page) {
                receive(*worker, page, before, canRetry);
            });
    connect(api, &Api::readTraceSignalErrorFull, this, [this](const Worker *worker) {
        finish(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    if (before > 0) {
        api->readTrace(traceId(), before);
    } else {
        api->readTrace(traceId());
    }
    emit changed();
}

void TraceInspector::receive(const Worker &worker, const Page &page, qint64 before, bool canRetry) {
    if (worker.getHttpResponseCode() != 200 || !validPage(page, traceId(), before)) {
        finish(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    if (canRetry && std::ranges::none_of(page.getSpans(), [](const Sample &span) {
            return !span.is_parent_span_id_Set();
        })) {
        request_.cancel();
        retry_.start(350);
        return;
    }
    publish(page);
    if (hasMore() && selectedId_ == 0 && !destination_.span.isEmpty() &&
        selected().spanId() != destination_.span) {
        load(beforeId_, false);
    } else {
        finish();
    }
}

void TraceInspector::publish(const Page &page) {
    spanSamples_.append(page.getSpans());
    outline_ = makeOutline(spanSamples_);
    for (const auto &log : page.getLogs()) {
        logs_.append(TraceRecord(log));
    }
    beforeId_ = page.is_next_before_id_Set() ? page.getNextBeforeId() : 0;
    emit recordsChanged();
    emit selectionChanged();
}

void TraceInspector::finish(const QString &feedback) {
    request_.cancel();
    busy_ = false;
    feedback_ = feedback;
    emit changed();
}

} // namespace ctxfab
