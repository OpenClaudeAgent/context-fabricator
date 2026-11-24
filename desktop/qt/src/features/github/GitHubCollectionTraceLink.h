#pragma once

#include "GitHubCollectionActivity.h"
#include "GitHubCollectionTrace.h"

#include <QCoreApplication>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class GitHubCollectionTraceLink {
    Q_GADGET
    QML_VALUE_TYPE(gitHubCollectionTraceLink)
    Q_DECLARE_TR_FUNCTIONS(GitHubCollectionTraceLink)

    Q_PROPERTY(QString label READ label CONSTANT FINAL)
    Q_PROPERTY(QString traceId READ traceId CONSTANT FINAL)
    Q_PROPERTY(QString spanId READ spanId CONSTANT FINAL)
    Q_PROPERTY(QString kind READ kind CONSTANT FINAL)

  public:
    GitHubCollectionTraceLink() = default;

    [[nodiscard]] QString label() const;
    [[nodiscard]] QString traceId() const { return trace_.getTraceId(); }
    [[nodiscard]] QString spanId() const { return trace_.getSpanId(); }
    [[nodiscard]] QString kind() const;
    [[nodiscard]] static QList<GitHubCollectionTraceLink>
    links(const openapi::GitHubCollectionActivity &activity);

  private:
    enum class Kind { Collection, Latest, Published };
    GitHubCollectionTraceLink(const openapi::GitHubCollectionTrace &trace, Kind kind);

    openapi::GitHubCollectionTrace trace_;
    Kind kind_ = Kind::Collection;
};

} // namespace ctxfab
