#pragma once

#include <QString>
#include <QUrl>

#include <optional>

namespace ctxfab {

struct Session {
    QUrl url;
    QString token;
    qint64 processId;

    static std::optional<Session> read(const QString &path);
};

} // namespace ctxfab
