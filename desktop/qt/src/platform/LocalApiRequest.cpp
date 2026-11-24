#include "LocalApiRequest.h"

#include "DefaultApi.h"
#include "Problem.h"
#include "Session.h"

#include <QNetworkProxy>
#include <QNetworkRequest>

namespace ctxfab {

LocalApiRequest::LocalApiRequest(int timeoutMs) : timeoutMs_(timeoutMs) {
    network_.setProxy(QNetworkProxy::NoProxy);
    network_.setRedirectPolicy(QNetworkRequest::ManualRedirectPolicy);
}

LocalApiRequest::~LocalApiRequest() { delete request_; }

openapi::DefaultApi *LocalApiRequest::begin(const Session &session, std::optional<int> timeoutMs) {
    cancel();
    request_ = new openapi::DefaultApi(timeoutMs.value_or(timeoutMs_));
    request_->setParent(&network_);
    request_->setNetworkAccessManager(&network_);
    request_->setNewServerForAllOperations(session.url, {}, {});
    request_->setBearerToken(session.token);
    request_->addHeaders(QStringLiteral("User-Agent"), QStringLiteral("ContextFabricator"));
    return request_;
}

void LocalApiRequest::cancel() {
    if (request_) {
        QObject::disconnect(request_, nullptr, nullptr, nullptr);
        request_->deleteLater();
        request_ = nullptr;
    }
}

QString LocalApiRequest::rejection(const openapi::HttpRequestWorker &worker,
                                   const QString &fallback) {
    const openapi::Problem problem(QString::fromUtf8(worker.response));
    return problem.isValid() ? problem.getMessage() : fallback;
}

} // namespace ctxfab
