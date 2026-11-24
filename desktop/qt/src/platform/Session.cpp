#include "Session.h"

#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QRegularExpression>
#include <QScopeGuard>

#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

namespace ctxfab {

namespace {

bool privateFileDescriptor(int descriptor, mode_t type) {
    struct stat metadata{};
    return ::fstat(descriptor, &metadata) == 0 && (metadata.st_mode & S_IFMT) == type &&
           metadata.st_uid == ::geteuid() && (metadata.st_mode & (S_IRWXG | S_IRWXO)) == 0;
}

QByteArray readPrivateEndpoint(const QFileInfo &path) {
    const auto directory = QFile::encodeName(path.absolutePath());
    const auto name = QFile::encodeName(path.fileName());
    const int parent =
        ::open(directory.constData(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (parent < 0) {
        return {};
    }
    const auto closeParent = qScopeGuard([parent] { ::close(parent); });
    if (!privateFileDescriptor(parent, S_IFDIR)) {
        return {};
    }

    const int descriptor =
        ::openat(parent, name.constData(), O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC);
    if (descriptor < 0) {
        return {};
    }
    const auto closeEndpoint = qScopeGuard([descriptor] { ::close(descriptor); });
    QFile file;
    if (!privateFileDescriptor(descriptor, S_IFREG) ||
        !file.open(descriptor, QIODevice::ReadOnly, QFileDevice::DontCloseHandle)) {
        return {};
    }
    constexpr qint64 MaxEndpointBytes = 4096;
    const auto bytes = file.read(MaxEndpointBytes + 1);
    return bytes.size() <= MaxEndpointBytes && file.error() == QFileDevice::NoError ? bytes
                                                                                    : QByteArray{};
}

bool validAddress(const QUrl &url, const QString &address) {
    return url.isValid() && url.port() > 0 &&
           address == QStringLiteral("http://127.0.0.1:%1").arg(url.port());
}

} // namespace

std::optional<Session> Session::read(const QString &path) {
    const auto endpoint = QJsonDocument::fromJson(readPrivateEndpoint(QFileInfo(path))).object();
    const auto address = endpoint.value("url").toString();
    const QUrl url(address, QUrl::StrictMode);
    const auto token = endpoint.value("token").toString();
    const auto processId = endpoint.value("processId").toInteger();
    static const QRegularExpression capability(QStringLiteral("\\A[0-9a-f]{64}\\z"));

    if (endpoint.value("apiVersion").toInt() != 1 || processId <= 0 ||
        !validAddress(url, address) || !capability.match(token).hasMatch()) {
        return std::nullopt;
    }
    return Session{url, token, processId};
}

} // namespace ctxfab
