#pragma once

#include "GitHubGraphOccurrence.h"
#include "GitHubGraphSnapshot.h"
#include "LocalApiRequest.h"

#include <QObject>
#include <QStringList>
#include <QtQml/qqmlregistration.h>

namespace ctxfab {

class BackendSupervisor;

namespace openapi {
class GitHubGraph;
}

class GitHubGraphBrowser final : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(GitHubGraphBrowser)
    QML_UNCREATABLE("Created by the desktop application")

    Q_PROPERTY(bool visible READ visible NOTIFY changed FINAL)
    Q_PROPERTY(bool busy READ busy NOTIFY changed FINAL)
    Q_PROPERTY(QString branch READ branch NOTIFY changed FINAL)
    Q_PROPERTY(Access access READ access NOTIFY changed FINAL)
    Q_PROPERTY(Coverage coverage READ coverage NOTIFY changed FINAL)
    Q_PROPERTY(QString feedback READ feedback NOTIFY changed FINAL)
    Q_PROPERTY(QString commit READ commit NOTIFY changed FINAL)
    Q_PROPERTY(QStringList roots READ roots NOTIFY changed FINAL)
    Q_PROPERTY(int rootIndex READ rootIndex NOTIFY changed FINAL)
    Q_PROPERTY(QString rootSummary READ rootSummary NOTIFY changed FINAL)
    Q_PROPERTY(QString searchedRoot READ searchedRoot NOTIFY changed FINAL)
    Q_PROPERTY(QString searchedSymbol READ searchedSymbol NOTIFY changed FINAL)
    Q_PROPERTY(QList<ctxfab::GitHubGraphOccurrence> hits READ hits NOTIFY changed FINAL)
    Q_PROPERTY(bool truncated READ truncated NOTIFY changed FINAL)

  public:
    enum Access { Unknown, Available, Retained, Unavailable };
    Q_ENUM(Access)
    enum Coverage { NoSnapshot, Published, NoConventionalRustRoots, Limit };
    Q_ENUM(Coverage)

    explicit GitHubGraphBrowser(BackendSupervisor &backend);

    [[nodiscard]] bool visible() const { return id_ > 0; }
    [[nodiscard]] bool busy() const { return busy_; }
    [[nodiscard]] QString branch() const { return branch_; }
    [[nodiscard]] Access access() const { return access_; }
    [[nodiscard]] Coverage coverage() const;
    [[nodiscard]] QString feedback() const;
    [[nodiscard]] QString commit() const { return snapshot_.getRevision().left(10); }
    [[nodiscard]] QStringList roots() const;
    [[nodiscard]] int rootIndex() const;
    [[nodiscard]] QString rootSummary() const;
    [[nodiscard]] QString searchedRoot() const { return search_.root; }
    [[nodiscard]] QString searchedSymbol() const { return search_.symbol; }
    [[nodiscard]] QList<GitHubGraphOccurrence> hits() const { return hits_; }
    [[nodiscard]] bool truncated() const { return snapshot_.isTruncated(); }

    void open(qint64 id, const QString &repository, const QString &branch);
    Q_INVOKABLE void close();
    Q_INVOKABLE void reload();
    Q_INVOKABLE void selectRoot(int index);
    Q_INVOKABLE void lookup(const QString &symbol);

  signals:
    void changed();

  private:
    struct Search {
        QString root;
        QString symbol;
    };

    void load(const Search &search);
    void receive(const openapi::HttpRequestWorker &worker, const openapi::GitHubGraph &graph,
                 const Search &search);
    void publish(const openapi::GitHubGraph &graph, const Search &search);
    void reset();
    void finish(const QString &feedback = {});

    BackendSupervisor &backend_;
    LocalApiRequest request_;
    qint64 id_ = 0;
    QString repository_;
    QString branch_;
    bool busy_ = false;
    Access access_ = Unknown;
    openapi::GitHubGraphSnapshot snapshot_;
    QString selectedRoot_;
    Search search_;
    QList<GitHubGraphOccurrence> hits_;
    QString feedback_;
};

} // namespace ctxfab
