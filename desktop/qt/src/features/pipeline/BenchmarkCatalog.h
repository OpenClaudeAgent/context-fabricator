#pragma once

#include "BenchmarkAction.h"
#include "BenchmarkActionDownload.h"
#include "BenchmarkDocument.h"
#include "BenchmarkReference.h"
#include "Question.h"

#include <QJsonObject>
#include <QList>
#include <QString>
#include <optional>

namespace ctxfab {

struct BenchmarkCase {
    QString query;
    QString expected;
    QString source;
    std::optional<openapi::BenchmarkAction> action = {};
};

struct BenchmarkSuite {
    QString id;
    QString title;
    std::optional<openapi::BenchmarkReference> reference = {};
    QList<BenchmarkCase> questions;
    QList<openapi::BenchmarkDocument> documents = {};
    bool ready = false;
    std::optional<openapi::Question> localScope = {};
};

struct BenchmarkCatalog {
    QList<BenchmarkSuite> corpora;
    QList<BenchmarkCase> captures;
    QJsonObject workflowPrompts;

    [[nodiscard]] static BenchmarkCatalog bundled();
    [[nodiscard]] BenchmarkSuite downloaded(const openapi::BenchmarkActionDownload &download) const;
    [[nodiscard]] static QString key(const openapi::BenchmarkReference &reference);
    [[nodiscard]] static QString key(const openapi::BenchmarkAction &action);
};

} // namespace ctxfab
