#pragma once

#include "DiagnosticLogs.h"

#include <QSortFilterProxyModel>

namespace ctxfab {

class Answers;

class DiagnosticAnswerRequests final : public QSortFilterProxyModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Created by the application")

    Q_PROPERTY(ctxfab::DiagnosticLogs *feed READ feed CONSTANT FINAL)
    Q_PROPERTY(QString latestTraceId READ latestTraceId NOTIFY latestChanged FINAL)
    Q_PROPERTY(QString latestQuestion READ latestQuestion NOTIFY latestChanged FINAL)
    Q_PROPERTY(bool latestAbstained READ latestAbstained NOTIFY latestChanged FINAL)
    Q_PROPERTY(bool pending READ pending NOTIFY latestChanged FINAL)

  public:
    DiagnosticAnswerRequests(BackendSupervisor &backend, Answers &answers);

    [[nodiscard]] QHash<int, QByteArray> roleNames() const override;
    [[nodiscard]] QVariant data(const QModelIndex &index, int role) const override;
    [[nodiscard]] DiagnosticLogs *feed() { return &feed_; }
    [[nodiscard]] const QString &latestTraceId() const { return latestTraceId_; }
    [[nodiscard]] QString latestQuestion() const;
    [[nodiscard]] bool latestAbstained() const;
    [[nodiscard]] bool pending() const;

  signals:
    void latestChanged();

  protected:
    [[nodiscard]] bool filterAcceptsRow(int sourceRow, const QModelIndex &parent) const override;

  private:
    void synchronize();
    void updateQuestions();

    static constexpr int QuestionRole = DiagnosticLogs::SampleRole + 1;
    Answers &answers_;
    DiagnosticLogs feed_;
    QString latestTraceId_;
};

} // namespace ctxfab
