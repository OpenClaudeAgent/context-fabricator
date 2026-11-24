#include "AppLanguage.h"

#include <QCoreApplication>
#include <QLocale>
#include <QSettings>
#include <QTranslator>

#include <array>
#include <utility>

namespace ctxfab {
namespace {

constexpr auto PreferenceKey = "app-language";
struct Language {
    const char *code;
    const char *name;
};

constexpr std::array Languages{
    Language{"en", "English"}, Language{"fr", "Français"},      Language{"de", "Deutsch"},
    Language{"es", "Español"}, Language{"it", "Italiano"},      Language{"pt", "Português"},
    Language{"ja", "日本語"},  Language{"zh-Hans", "简体中文"},
};

int selectionFor(const QString &code) {
    for (std::size_t index = 0; index < Languages.size(); ++index) {
        if (code == QLatin1StringView(Languages.at(index).code)) {
            return static_cast<int>(index + 1);
        }
    }
    return 0;
}

QString systemLanguage(const QStringList &preferredLanguages) {
    for (const auto &tag : preferredLanguages) {
        const QLocale locale(tag);
        if (locale.language() == QLocale::Chinese) {
            if (locale.script() == QLocale::SimplifiedHanScript) {
                return QStringLiteral("zh-Hans");
            }
            continue;
        }
        const auto code = QLocale::languageToCode(locale.language());
        if (selectionFor(code) != 0) {
            return code;
        }
    }
    return QStringLiteral("en");
}

QString languageFor(int selection, const QString &systemCode) {
    return selection == 0
               ? systemCode
               : QString::fromLatin1(Languages.at(static_cast<std::size_t>(selection - 1)).code);
}

} // namespace

AppLanguage::AppLanguage(QSettings &settings, const QStringList &preferredLanguages)
    : settings_(settings), systemLanguage_(systemLanguage(preferredLanguages)) {
    const int restored = selectionFor(settings_.value(QLatin1StringView(PreferenceKey)).toString());
    if (apply(languageFor(restored, systemLanguage_))) {
        selection_ = restored;
    }
}

AppLanguage::~AppLanguage() {
    if (translator_) {
        QCoreApplication::removeTranslator(translator_.get());
    }
}

QStringList AppLanguage::options() const {
    QStringList result{tr("System Default")};
    for (const auto &language : Languages) {
        result.append(QString::fromUtf8(language.name));
    }
    return result;
}

void AppLanguage::setSelection(int selection) {
    if (selection < 0 || static_cast<std::size_t>(selection) > Languages.size() ||
        selection == selection_ || !apply(languageFor(selection, systemLanguage_))) {
        return;
    }
    selection_ = selection;
    settings_.setValue(QLatin1StringView(PreferenceKey),
                       languageFor(selection_, QStringLiteral("system")));
    emit changed();
}

bool AppLanguage::apply(const QString &code) {
    if (code == effectiveLanguage_) {
        return true;
    }
    std::unique_ptr<QTranslator> next;
    if (code != QStringLiteral("en")) {
        next = std::make_unique<QTranslator>();
        if (!next->load(QStringLiteral(":/i18n/ctxfab_%1.qm").arg(code)) ||
            !QCoreApplication::installTranslator(next.get())) {
            qWarning("Context Fabricator could not load its application translation.");
            return false;
        }
    }
    if (translator_) {
        QCoreApplication::removeTranslator(translator_.get());
    }
    translator_ = std::move(next);
    effectiveLanguage_ = code;
    return true;
}

} // namespace ctxfab
