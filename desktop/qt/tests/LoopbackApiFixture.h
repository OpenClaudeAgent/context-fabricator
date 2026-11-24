#pragma once

#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QTcpServer>
#include <QTcpSocket>
#include <QTemporaryDir>
#include <QTimer>

#include <optional>

namespace ctxfab::test {

struct Response {
    QJsonObject body;
    QByteArray status = "200 OK";
    std::optional<QJsonArray> array = {};
    int delayMs = 0;
};

class LoopbackApiFixture final {
  public:
    LoopbackApiFixture() {
        QObject::connect(&server_, &QTcpServer::newConnection, &server_, [this] {
            auto *socket = server_.nextPendingConnection();
            QObject::connect(socket, &QTcpSocket::readyRead, socket, [this, socket] {
                const auto request = socket->property("request").toByteArray() + socket->readAll();
                const auto separator = request.indexOf("\r\n\r\n");
                if (separator >= 0 &&
                    request.size() >= separator + 4 + bodySize(request.left(separator))) {
                    const auto requestLine = request.split(' ');
                    respond(*socket, requestLine.value(0), requestLine.value(1),
                            request.mid(separator + 4));
                } else {
                    socket->setProperty("request", request);
                }
            });
            QObject::connect(socket, &QTcpSocket::disconnected, socket, &QObject::deleteLater);
        });
    }

    bool start() {
        if (!directory_.isValid() || !server_.listen(QHostAddress::LocalHost)) {
            return false;
        }
        QFile endpoint(endpointPath());
        const QJsonDocument document(QJsonObject{
            {"apiVersion", 1},
            {"processId", 123},
            {"url", QStringLiteral("http://127.0.0.1:%1").arg(server_.serverPort())},
            {"token", QString(64, 'a')},
        });
        const auto bytes = document.toJson();
        return endpoint.open(QIODevice::WriteOnly) &&
               endpoint.setPermissions(QFileDevice::ReadOwner | QFileDevice::WriteOwner) &&
               endpoint.write(bytes) == bytes.size();
    }

    [[nodiscard]] QString endpointPath() const { return directory_.filePath("endpoint.json"); }

    QHash<QByteArray, QList<Response>> replies;
    QList<QByteArray> requests;
    QHash<QByteArray, QJsonObject> requestBodies;

  private:
    static qsizetype bodySize(const QByteArray &headers) {
        const auto lines = headers.split('\n');
        for (const auto &line : lines) {
            if (line.toLower().startsWith("content-length:")) {
                return line.sliced(15).trimmed().toLongLong();
            }
        }
        return 0;
    }

    void respond(QTcpSocket &socket, const QByteArray &method, const QByteArray &route,
                 const QByteArray &body) {
        requests.append(route);
        const auto request = method + ' ' + route;
        if (!body.isEmpty()) {
            requestBodies.insert(request, QJsonDocument::fromJson(body).object());
        }
        auto specific = replies.find(request);
        if (specific == replies.end() && request.startsWith("DELETE /v1/answers/")) {
            specific = replies.find("DELETE /v1/answers/*");
        }
        auto &sequence = specific == replies.end() ? replies[route] : specific.value();
        if (sequence.isEmpty()) {
            socket.disconnectFromHost();
            return;
        }
        const auto reply = sequence.size() > 1 ? sequence.takeFirst() : sequence.constFirst();
        const auto document = reply.array ? QJsonDocument(*reply.array) : QJsonDocument(reply.body);
        const auto json = document.toJson(QJsonDocument::Compact);
        const auto header = "HTTP/1.1 " + reply.status + "\r\nContent-Type: application/json\r\n" +
                            "Content-Length: " + QByteArray::number(json.size()) +
                            "\r\nConnection: close\r\n\r\n";
        const auto write = [&socket, response = header + json] {
            socket.write(response);
            socket.disconnectFromHost();
        };
        if (reply.delayMs > 0) {
            QTimer::singleShot(reply.delayMs, &socket, write);
        } else {
            write();
        }
    }

    QTemporaryDir directory_;
    QTcpServer server_;
};

} // namespace ctxfab::test
