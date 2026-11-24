#pragma once

#include "AnswerUsage.h"
#include "LocalApiRequest.h"

#include <QCoreApplication>
#include <QObject>
#include <QtQmlIntegration/qqmlintegration.h>

#include <optional>

namespace ctxfab {

class BackendSupervisor;

class AnswerUsageRow {
    Q_GADGET
    QML_VALUE_TYPE(answerUsageRow)
    Q_DECLARE_TR_FUNCTIONS(AnswerUsageRow)

    Q_PROPERTY(QString client READ client CONSTANT FINAL)
    Q_PROPERTY(QString profile READ profile CONSTANT FINAL)
    Q_PROPERTY(int published READ published CONSTANT FINAL)
    Q_PROPERTY(int abstained READ abstained CONSTANT FINAL)
    Q_PROPERTY(int cancelled READ cancelled CONSTANT FINAL)
    Q_PROPERTY(int failed READ failed CONSTANT FINAL)
    Q_PROPERTY(int unknown READ unknown CONSTANT FINAL)
    Q_PROPERTY(QString median READ median CONSTANT FINAL)
    Q_PROPERTY(QString p95 READ p95 CONSTANT FINAL)
    Q_PROPERTY(QString traceId READ traceId CONSTANT FINAL)

  public:
    AnswerUsageRow() = default;
    explicit AnswerUsageRow(const openapi::AnswerUsageGroup &group) : group_(group) {}

    [[nodiscard]] QString client() const;
    [[nodiscard]] QString profile() const;
    [[nodiscard]] int published() const { return group_.getPublished(); }
    [[nodiscard]] int abstained() const { return group_.getAbstained(); }
    [[nodiscard]] int cancelled() const { return group_.getCancelled(); }
    [[nodiscard]] int failed() const { return group_.getFailed(); }
    [[nodiscard]] int unknown() const { return group_.getUnknown(); }
    [[nodiscard]] QString median() const;
    [[nodiscard]] QString p95() const;
    [[nodiscard]] QString traceId() const { return group_.getLatestTraceId(); }

  private:
    openapi::AnswerUsageGroup group_;
};

class AnswerUsageModel final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")
    Q_PROPERTY(bool ready READ ready NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(bool hasSnapshot READ hasSnapshot NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString period READ period NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::AnswerUsageRow> groups READ groups NOTIFY changed FINAL)

  public:
    explicit AnswerUsageModel(BackendSupervisor &backend);

    [[nodiscard]] bool ready() const { return ready_; }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] bool hasSnapshot() const { return snapshot_.has_value(); }
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] QString period() const;
    [[nodiscard]] QList<AnswerUsageRow> groups() const;

    Q_INVOKABLE void setActive(bool active);
    Q_INVOKABLE void refresh();

  signals:
    void changed();

  private:
    void backendChanged();
    void finish(const QString &message);

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    std::optional<openapi::AnswerUsage> snapshot_;
    QString feedback_;
    bool active_ = false;
    bool ready_ = false;
    bool busy_ = false;
};

} // namespace ctxfab
