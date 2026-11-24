#pragma once

#include "Session.h"

#include <QObject>
#include <QProcess>
#include <QTimer>

namespace ctxfab {

class BackendSupervisor final : public QObject {
    Q_OBJECT

  public:
    struct Options {
        QString executable;
        QString directory;
        QString externalEndpoint;
    };

    enum class State { Waiting, Starting, Available, Restarting, Failed };

    explicit BackendSupervisor(Options options);
    ~BackendSupervisor() override;

    static QString defaultDataDirectory();

    bool managed() const { return options_.externalEndpoint.isEmpty(); }
    bool canRestart() const;
    State state() const { return state_; }
    QString error() const;
    QString profileDirectory() const;
    std::optional<Session> session() const;

    void start();
    void restart();

  signals:
    void changed();

  private:
    void launch();
    void readChildEndpoint();
    void childFinished(int code, QProcess::ExitStatus exitStatus);
    void setState(State state, const char *error = nullptr);

    Options options_;
    QProcess process_;
    QTimer poll_;
    QTimer startDeadline_;
    QTimer stopDeadline_;
    State state_ = State::Waiting;
    const char *error_ = nullptr;
    std::optional<Session> session_;
};

} // namespace ctxfab
