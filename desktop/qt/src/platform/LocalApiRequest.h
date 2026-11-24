#pragma once

#include <QNetworkAccessManager>

#include <optional>

namespace ctxfab {

struct Session;

namespace openapi {
class DefaultApi;
class HttpRequestWorker;
} // namespace openapi

class LocalApiRequest final {
  public:
    explicit LocalApiRequest(int timeoutMs = 3000);
    ~LocalApiRequest();

    openapi::DefaultApi *begin(const Session &session, std::optional<int> timeoutMs = {});
    void cancel();

    static QString rejection(const openapi::HttpRequestWorker &worker, const QString &fallback);

  private:
    QNetworkAccessManager network_;
    int timeoutMs_;
    openapi::DefaultApi *request_ = nullptr;
};

} // namespace ctxfab
