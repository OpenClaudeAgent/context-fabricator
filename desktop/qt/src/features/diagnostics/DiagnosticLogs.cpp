#include "DiagnosticLogs.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <QJsonDocument>

#include <algorithm>

namespace ctxfab {
namespace {

using Api = openapi::DefaultApi;
using Worker = openapi::HttpRequestWorker;
using Page = openapi::DiagnosticLogPage;
using LogLevel = openapi::DiagnosticLogLevel;
using Sample = openapi::SignalSample;
using openapi::OptionalParam;

constexpr int MaxQueryCharacters = 128;
constexpr int DebounceMilliseconds = 300;

OptionalParam<LogLevel> filterLevel(int level) {
    using Value = LogLevel::eDiagnosticLogLevel;
    LogLevel result;
    switch (level) {
    case DiagnosticLogs::Info:
        result.setValue(Value::INFO);
        break;
    case DiagnosticLogs::Error:
        result.setValue(Value::ERROR);
        break;
    case DiagnosticLogs::Debug:
        result.setValue(Value::DEBUG);
        break;
    default:
        return {};
    }
    return result;
}

bool validRecords(const QList<Sample> &samples, std::optional<qint64> before) {
    for (const auto &sample : samples) {
        const auto id = sample.getId();
        if (id < 1 || (before && id >= *before)) {
            return false;
        }
        before = id;
    }
    return true;
}

bool validCursor(const QList<Sample> &samples, qint64 next) {
    return !samples.isEmpty() && next > 0 && next <= samples.constLast().getId();
}

bool isAnswerRequest(const Sample &sample) {
    return sample.getName() == "api_answer" && sample.getLevel() != "debug" &&
           TraceRecord::isValidTraceId(sample.getTraceId());
}

QList<TraceRecord> visibleRecords(const QList<Sample> &samples, DiagnosticLogs::Feed feed) {
    QList<TraceRecord> records;
    records.reserve(samples.size());
    for (const auto &sample : samples) {
        if (feed == DiagnosticLogs::Feed::ApplicationLogs || isAnswerRequest(sample)) {
            records.append(TraceRecord(sample));
        }
    }
    return records;
}

} // namespace

DiagnosticLogs::DiagnosticLogs(BackendSupervisor &backend, Feed feed)
    : backend_(backend), feed_(feed),
      query_(feed == Feed::AnswerRequests ? QStringLiteral("api_answer") : QString{}) {
    debounce_.setSingleShot(true);
    debounce_.setInterval(DebounceMilliseconds);
    connect(&debounce_, &QTimer::timeout, this, [this] { loadPage(); });
    connect(&backend_, &BackendSupervisor::changed, this, &DiagnosticLogs::backendChanged);
}

int DiagnosticLogs::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(records_.size());
}

QVariant DiagnosticLogs::data(const QModelIndex &index, int role) const {
    if (index.isValid() && role == SampleRole && index.row() < rowCount()) {
        return QVariant::fromValue(records_.at(index.row()));
    }
    return {};
}

QHash<int, QByteArray> DiagnosticLogs::roleNames() const { return {{SampleRole, "sample"}}; }

void DiagnosticLogs::retranslate() {
    if (!records_.isEmpty()) {
        emit dataChanged(index(0), index(rowCount() - 1), {SampleRole});
    }
    emit eventsChanged();
    emit changed();
}

QString DiagnosticLogs::feedback() const {
    if (!pageFeedback_.isEmpty()) {
        return tr(qUtf8Printable(pageFeedback_));
    }
    return level_ == AllLevels ? tr(qUtf8Printable(eventsFeedback_)) : QString{};
}

QList<DiagnosticEvent> DiagnosticLogs::events() const {
    if (level_ != AllLevels) {
        return {};
    }
    const auto needle = query_.trimmed();
    if (needle.isEmpty()) {
        return events_;
    }
    QList<DiagnosticEvent> matches;
    for (const auto &event : events_) {
        if (event.title().contains(needle, Qt::CaseInsensitive) ||
            event.code().contains(needle, Qt::CaseInsensitive)) {
            matches.append(event);
        }
    }
    return matches;
}

void DiagnosticLogs::setActive(bool active) {
    if (active_ == active) {
        return;
    }
    active_ = active;
    if (active_) {
        refresh();
    } else {
        cancelPage();
        eventRequest_.cancel();
        emit changed();
    }
}

void DiagnosticLogs::setLevel(int level) {
    if (feed_ != Feed::ApplicationLogs || level < AllLevels || level > Debug || level_ == level) {
        return;
    }
    level_ = static_cast<Level>(level);
    cancelPage();
    resetPage();
    emit changed();
    refresh();
    emit eventsChanged();
}

void DiagnosticLogs::setQuery(const QString &query) {
    if (feed_ != Feed::ApplicationLogs || query_ == query) {
        return;
    }
    query_ = query;
    cancelPage();
    resetPage();
    pageFeedback_.clear();
    if (active_ && ready_) {
        busy_ = true;
        debounce_.start();
    }
    emit eventsChanged();
    emit changed();
}

void DiagnosticLogs::refresh() {
    if (active_) {
        if (feed_ == Feed::ApplicationLogs && level_ == AllLevels) {
            loadEvents();
        }
        loadPage();
    }
}

void DiagnosticLogs::loadMore() {
    if (active_ && !busy_ && before_) {
        loadPage(before_);
    }
}

void DiagnosticLogs::backendChanged() {
    cancelPage();
    eventRequest_.cancel();
    resetPage();
    events_.clear();
    pageFeedback_.clear();
    eventsFeedback_.clear();
    ready_ = backend_.session().has_value();
    emit eventsChanged();
    emit changed();
    refresh();
}

void DiagnosticLogs::resetPage() {
    beginResetModel();
    records_.clear();
    before_.reset();
    endResetModel();
}

void DiagnosticLogs::cancelPage() {
    debounce_.stop();
    pageRequest_.cancel();
    busy_ = false;
}

void DiagnosticLogs::loadPage(std::optional<qint64> before) {
    debounce_.stop();
    const auto session = backend_.session();
    ready_ = session.has_value();
    if (!session) {
        finishPage(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    const auto query = query_.trimmed();
    if (query.toUcs4().size() > MaxQueryCharacters) {
        finishPage(QT_TR_NOOP("Enter a log query of up to 128 characters."));
        return;
    }
    auto *api = pageRequest_.begin(*session);
    connect(api, &Api::readTelemetryLogsSignalFull, this,
            [this, before](const Worker *worker, const Page &page) {
                acceptPage(*worker, page, before);
            });
    connect(api, &Api::readTelemetryLogsSignalErrorFull, this, [this](const Worker *worker) {
        finishPage(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    pageFeedback_.clear();
    busy_ = true;
    api->readTelemetryLogs(
        query.isEmpty() ? OptionalParam<QString>{} : OptionalParam<QString>(query),
        filterLevel(level_), before ? OptionalParam<qint64>(*before) : OptionalParam<qint64>{});
    emit changed();
}

void DiagnosticLogs::acceptPage(const Worker &worker, const Page &page,
                                std::optional<qint64> before) {
    const auto samples = page.getRecords();
    const auto next = page.getNextBeforeId();
    const auto more = page.is_next_before_id_Set();
    if (worker.getHttpResponseCode() != 200 || !page.isValid() || !validRecords(samples, before) ||
        (more && !validCursor(samples, next))) {
        finishPage(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    auto visible = visibleRecords(samples, feed_);
    if (!before) {
        beginResetModel();
        records_ = std::move(visible);
        endResetModel();
    } else if (!visible.isEmpty()) {
        beginInsertRows({}, rowCount(), rowCount() + static_cast<int>(visible.size()) - 1);
        records_.append(std::move(visible));
        endInsertRows();
    }
    before_ = more ? std::optional<qint64>(next) : std::nullopt;
    finishPage({});
}

void DiagnosticLogs::finishPage(const QString &message) {
    cancelPage();
    pageFeedback_ = message;
    emit changed();
}

void DiagnosticLogs::loadEvents() {
    const auto session = backend_.session();
    if (!session) {
        return;
    }
    auto *api = eventRequest_.begin(*session);
    connect(api, &Api::readDiagnosticsSignalFull, this,
            [this](const Worker *worker, const QList<openapi::Diagnostic> &events) {
                if (worker->getHttpResponseCode() != 200 ||
                    !QJsonDocument::fromJson(worker->response).isArray() ||
                    !std::ranges::all_of(events, &openapi::Diagnostic::isValid)) {
                    finishEvents(QT_TR_NOOP("Invalid response from the backend"));
                    return;
                }
                events_.clear();
                for (const auto &event : events) {
                    events_.append(DiagnosticEvent(event));
                }
                finishEvents({});
                emit eventsChanged();
            });
    connect(api, &Api::readDiagnosticsSignalErrorFull, this, [this](const Worker *worker) {
        finishEvents(LocalApiRequest::rejection(*worker, QT_TR_NOOP("Backend request failed")));
    });
    eventsFeedback_.clear();
    api->readDiagnostics();
}

void DiagnosticLogs::finishEvents(const QString &message) {
    eventRequest_.cancel();
    eventsFeedback_ = message;
    emit changed();
}

} // namespace ctxfab
