#include "Preferences.h"

#include "BackendSupervisor.h"
#include "DefaultApi.h"

#include <algorithm>
#include <utility>

namespace ctxfab {

namespace {

QString keepEdit(const QString &fresh, const QString &old, const QString &edit) {
    return old == edit ? fresh : edit;
}

bool within(const QString &text, int minimum, int maximum) {
    const auto value = text.toInt();
    return value >= minimum && value <= maximum;
}

openapi::AnswerLanguageOption option(const QString &code, const QString &name,
                                     const QString &nativeName) {
    openapi::AnswerLanguageOption result;
    result.setCode(code);
    result.setName(name);
    result.setNativeName(nativeName);
    return result;
}

} // namespace

Preferences::Draft Preferences::Draft::from(const openapi::Settings &settings) {
    return {.language = settings.is_answer_locale_Set() ? settings.getAnswerLocale()
                                                        : settings.getAnswerLanguage(),
            .retentionHours = QString::number(settings.getDiagnosticRetentionHours()),
            .diagnosticLimit = settings.is_diagnostic_storage_limit_mb_Set()
                                   ? QString::number(settings.getDiagnosticStorageLimitMb())
                                   : QString{},
            .artifactLimit = settings.is_inference_artifact_limit_gb_Set()
                                 ? QString::number(settings.getInferenceArtifactLimitGb())
                                 : QString{}};
}

void Preferences::Draft::preserveEdits(const Draft &old, const Draft &edits) {
    language = keepEdit(language, old.language, edits.language);
    retentionHours = keepEdit(retentionHours, old.retentionHours, edits.retentionHours);
    diagnosticLimit = keepEdit(diagnosticLimit, old.diagnosticLimit, edits.diagnosticLimit);
    artifactLimit = keepEdit(artifactLimit, old.artifactLimit, edits.artifactLimit);
}

Preferences::Preferences(BackendSupervisor &backend) : backend_(backend) {
    connect(&backend_, &BackendSupervisor::changed, this, &Preferences::backendChanged);
}

bool Preferences::hasDiagnosticLimit() const {
    return confirmed_ && confirmed_->is_diagnostic_storage_limit_mb_Set();
}

bool Preferences::hasArtifactLimit() const {
    return confirmed_ && confirmed_->is_inference_artifact_limit_gb_Set();
}

void Preferences::setRetentionHours(const QString &value) {
    if (ready_ && !busy_ && draft_.retentionHours != value) {
        draft_.retentionHours = value;
        emit changed();
    }
}

void Preferences::setDiagnosticLimit(const QString &value) {
    if (ready_ && !busy_ && draft_.diagnosticLimit != value) {
        draft_.diagnosticLimit = value;
        emit changed();
    }
}

void Preferences::setArtifactLimit(const QString &value) {
    if (ready_ && !busy_ && draft_.artifactLimit != value) {
        draft_.artifactLimit = value;
        emit changed();
    }
}

void Preferences::setLanguage(const QString &value) {
    if (ready_ && !busy_ && draft_.language != value) {
        draft_.language = value;
        emit changed();
    }
}

QList<openapi::AnswerLanguageOption> Preferences::options() const {
    const auto provided = confirmed_ ? confirmed_->getAnswerLanguageOptions()
                                     : QList<openapi::AnswerLanguageOption>{};
    if (!provided.isEmpty()) {
        return provided;
    }
    return {option("auto", tr("Automatic"), tr("Automatic")),
            option("en", tr("English"), "English"),
            option("fr", tr("French"), QStringLiteral("Français"))};
}

QString Preferences::languageName() const {
    for (const auto &item : options()) {
        if (item.getCode() == draft_.language) {
            return item.getCode() == QStringLiteral("auto") ? tr("Automatic")
                                                            : item.getNativeName();
        }
    }
    return draft_.language;
}

QList<openapi::AnswerLanguageOption> Preferences::filteredOptions() const {
    auto filtered = options();
    const auto query = languageFilter_.trimmed();
    filtered.removeIf([&query](const auto &item) {
        return !item.getName().contains(query, Qt::CaseInsensitive) &&
               !item.getNativeName().contains(query, Qt::CaseInsensitive) &&
               !item.getCode().contains(query, Qt::CaseInsensitive);
    });
    return filtered;
}

QStringList Preferences::languageChoices() const {
    QStringList choices;
    for (const auto &item : filteredOptions()) {
        choices.push_back(item.getCode() == QStringLiteral("auto") ? tr("Automatic")
                                                                   : item.getNativeName());
    }
    return choices;
}

void Preferences::chooseLanguage(int index) {
    const auto filtered = filteredOptions();
    if (index >= 0 && index < filtered.size()) {
        setLanguage(filtered.at(index).getCode());
    }
}

void Preferences::setLanguageFilter(const QString &value) {
    if (languageFilter_ != value) {
        languageFilter_ = value;
        emit languageChoicesChanged();
    }
}

void Preferences::open() {
    if (!opened_) {
        opened_ = true;
        reload();
    }
}

void Preferences::backendChanged() {
    request_.cancel();
    busy_ = false;
    ready_ = false;
    if (opened_) {
        reload();
    }
}

void Preferences::reload() {
    if (busy_) {
        return;
    }
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    busy_ = true;
    feedback_.clear();
    auto *api = request_.begin(*session);
    connect(api, &openapi::DefaultApi::readSettingsSignalFull, this,
            [this](const openapi::HttpRequestWorker *worker, const openapi::Settings &settings) {
                accept(*worker, settings, false);
            });
    connect(api, &openapi::DefaultApi::readSettingsSignalErrorFull, this,
            [this](const openapi::HttpRequestWorker *worker) { failed(*worker); });
    api->readSettings();
    emit changed();
}

void Preferences::save() {
    if (busy_ || !ready_ || conflict_) {
        return;
    }
    const auto inputError = validate();
    if (!inputError.isEmpty()) {
        finish(inputError);
        return;
    }
    const auto session = backend_.session();
    if (!session) {
        finish(QT_TR_NOOP("Backend session unavailable"));
        return;
    }
    busy_ = true;
    feedback_.clear();
    auto *api = request_.begin(*session);
    connect(api, &openapi::DefaultApi::saveSettingsSignalFull, this,
            [this](const openapi::HttpRequestWorker *worker, const openapi::Settings &settings) {
                accept(*worker, settings, true);
            });
    connect(api, &openapi::DefaultApi::saveSettingsSignalErrorFull, this,
            [this](const openapi::HttpRequestWorker *worker) { failed(*worker); });
    api->saveSettings(payload());
    emit changed();
}

QString Preferences::validate() const {
    if (!within(draft_.retentionHours, 1, 168)) {
        return QT_TR_NOOP("Diagnostic retention must be between 1 and 168 hours.");
    }
    if (hasDiagnosticLimit() && !within(draft_.diagnosticLimit, 2, 1000)) {
        return QT_TR_NOOP("Diagnostic storage limit must be between 2 and 1000 MB.");
    }
    if (hasArtifactLimit() && !within(draft_.artifactLimit, 1, 1000)) {
        return QT_TR_NOOP("Inference artifact limit must be between 1 and 1000 GB.");
    }
    if (!std::ranges::any_of(
            options(), [this](const auto &item) { return item.getCode() == draft_.language; })) {
        return QT_TR_NOOP("Choose a supported answer language.");
    }
    return {};
}

openapi::Settings Preferences::payload() const {
    openapi::Settings settings;
    settings.setRevision(confirmed_->getRevision());
    const auto legacyLanguage =
        draft_.language == QStringLiteral("en") || draft_.language == QStringLiteral("fr")
            ? draft_.language
            : QStringLiteral("auto");
    settings.setAnswerLanguage(legacyLanguage);
    if (confirmed_->is_answer_locale_Set() || confirmed_->is_answer_language_options_Set()) {
        settings.setAnswerLocale(draft_.language);
    }
    settings.setDiagnosticRetentionHours(draft_.retentionHours.toInt());
    if (hasDiagnosticLimit()) {
        settings.setDiagnosticStorageLimitMb(draft_.diagnosticLimit.toInt());
    }
    if (hasArtifactLimit()) {
        settings.setInferenceArtifactLimitGb(draft_.artifactLimit.toInt());
    }
    return settings;
}

void Preferences::accept(const openapi::HttpRequestWorker &worker,
                         const openapi::Settings &settings, bool saved) {
    if (worker.getHttpResponseCode() != 200 || !settings.isValid() || settings.getRevision() < 0) {
        finish(QT_TR_NOOP("Invalid response from the backend"));
        return;
    }
    auto fresh = Draft::from(settings);
    if (confirmed_ && !saved) {
        fresh.preserveEdits(Draft::from(*confirmed_), draft_);
    }
    confirmed_ = settings;
    draft_ = std::move(fresh);
    ready_ = true;
    conflict_ = false;
    emit languageChoicesChanged();
    finish(saved ? QT_TR_NOOP("Settings saved.") : QString{});
}

void Preferences::failed(const openapi::HttpRequestWorker &worker) {
    if (worker.getHttpResponseCode() == 409) {
        conflict_ = true;
        finish(QT_TR_NOOP("Settings changed. Reload and review your edits before saving."));
        return;
    }
    finish(LocalApiRequest::rejection(worker, QT_TR_NOOP("Backend request failed")));
}

QString Preferences::feedback() const { return tr(qUtf8Printable(feedback_)); }

void Preferences::finish(const QString &message) {
    request_.cancel();
    busy_ = false;
    feedback_ = message;
    emit changed();
}

} // namespace ctxfab
