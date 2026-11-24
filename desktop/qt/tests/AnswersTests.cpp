#include "AnswerHistory.h"
#include "Answers.h"
#include "BackendSupervisor.h"
#include "DiagnosticAnswerRequests.h"
#include "HistoryPreferences.h"
#include "TraceAnswer.h"

#include <QDir>
#include <QFile>
#include <QProcess>
#include <QTemporaryDir>
#include <QtTest>

namespace {

constexpr auto ModelFile = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
constexpr auto Question = "What is the retry delay?";
constexpr auto ExpiringTrace = "abababababababababababababababab";

bool seedCorpus(const QString &path) {
    return QProcess::execute("python3", {"-c", QStringLiteral(R"(
import sqlite3
import sys
with sqlite3.connect(sys.argv[1]) as database:
    database.executescript("""
        INSERT INTO collections(name) VALUES ('qt-answer-fixture');
        INSERT INTO documents(collection, source, revision, kind, title, content)
        VALUES ('qt-answer-fixture', 'src/outbound.rs', 'r-native', 'history',
                'Retry cooldown', 'The retry cooldown is 47 seconds.');
    """)
)"),
                                         path}) == 0;
}

bool provisionWorker(const QDir &directory) {
    const auto worker = directory.filePath("inference/llama-server");
    QFile model(directory.filePath(QStringLiteral("inference/") + ModelFile));
    return directory.mkdir("inference") &&
           QFile::copy(QStringLiteral(CTXFAB_ANSWER_FIXTURE), worker) &&
           QFile::setPermissions(worker, QFile::ReadOwner | QFile::WriteOwner | QFile::ExeOwner) &&
           model.open(QIODevice::WriteOnly);
}

bool populateOlderHistory(const QString &path) {
    return QProcess::execute("python3", {"-c", QStringLiteral(R"(
import sqlite3
import sys
with sqlite3.connect(sys.argv[1]) as database:
    database.executemany("""
        INSERT INTO answer_history_entries (id, question, scope_json, completed_at_ms,
            answer_version, answer_json, outcome, trace_id)
        SELECT ?, ?, CASE
            WHEN ? = 25 THEN '{"repositoryId":9007199254740993,"branch":"main","actionsRunId":9007199254740994,"actionsRunAttempt":5}'
            ELSE scope_json END, completed_at_ms, answer_version, answer_json, outcome, ?
        FROM answer_history_entries WHERE id = 1
    """, [(9007199254741000 + index, f"History item {index}", index,
           bytes.fromhex("ab" * 16) if index == 25 else None) for index in range(26)])
)"),
                                         path}) == 0;
}

bool makeNewestFixtureExpire(const QString &path) {
    return QProcess::execute("python3", {"-c", QStringLiteral(R"(
import sqlite3
import sys
import time
with sqlite3.connect(sys.argv[1]) as database:
    database.execute("UPDATE answer_history_entries SET completed_at_ms = ? WHERE id = 9007199254741025",
        (int(time.time() * 1000) - 86_400_000 + 1_300,))
)"),
                                         path}) == 0;
}

} // namespace

class AnswersTests : public QObject {
    Q_OBJECT

  private slots:
    void opensRetainedAnswersAndClearsSessionHistory() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the Rust backend");
        }
        const QTemporaryDir profile;
        QVERIFY(profile.isValid());
        const QDir directory(profile.path());
        QVERIFY(provisionWorker(directory));
        ctxfab::BackendSupervisor backend(
            {.executable = executable, .directory = profile.path(), .externalEndpoint = {}});
        ctxfab::HistoryPreferences preferences(backend);
        ctxfab::AnswerHistory history(backend, preferences);
        ctxfab::Answers answers(backend);
        ctxfab::TraceAnswer traced(backend, answers, preferences);
        connect(&answers, &ctxfab::Answers::requestFinished, &history,
                &ctxfab::AnswerHistory::refresh);
        connect(&preferences, &ctxfab::HistoryPreferences::historyCleared, &answers,
                &ctxfab::Answers::discardSessionQuestions);

        backend.start();
        QTRY_VERIFY_WITH_TIMEOUT(answers.ready(), 8000);
        QTRY_VERIFY(!answers.activeModel().isEmpty());
        history.open();
        QTRY_VERIFY(preferences.ready());
        QVERIFY(!history.savingEnabled());
        const auto database = directory.filePath("context.sqlite3");
        QVERIFY(seedCorpus(database));
        answers.setQuestion(Question);
        answers.ask();
        QTRY_VERIFY_WITH_TIMEOUT(!answers.busy(), 7000);
        QCOMPARE(answers.sessionQuestions().size(), 1);
        QCOMPARE(answers.sessionQuestions().first().text(), QString(Question));
        QCOMPARE(answers.sessionQuestions().first().traceId(), answers.traceId());
        QCOMPARE(answers.sessionQuestions().first().outcome(), QStringLiteral("Answered"));
        QCOMPARE(history.rowCount(), 0);

        preferences.setEnabled(true);
        QTRY_VERIFY(history.savingEnabled());
        QTRY_VERIFY(!history.loading());
        answers.ask();
        QTRY_COMPARE_WITH_TIMEOUT(history.rowCount(), 1, 7000);
        const auto persistedTrace = answers.traceId();
        traced.inspect(persistedTrace, true);
        QVERIFY(traced.available());
        QVERIFY(traced.live());
        QCOMPARE(traced.question(), QString(Question));
        QVERIFY(traced.result().text().contains("47 seconds"));
        QVERIFY(traced.details().sources().constFirst().parts().constFirst().text().contains(
            "47 seconds"));
        const auto role = history.roleNames().key("entry");
        const auto summary = history.data(history.index(0), role).value<ctxfab::HistorySummary>();
        QCOMPARE(summary.question(), QString(Question));
        QVERIFY(!summary.completedAt().isEmpty());
        QVERIFY(!history.data(history.index(99), role).isValid());
        history.select(0);
        QTRY_VERIFY(history.savedAnswer().present());
        QCOMPARE(history.savedAnswer().traceId(), persistedTrace);
        QVERIFY(!history.savedAnswer().completedAt().isEmpty());
        QVERIFY(history.savedAnswer().result().text().contains("47 seconds"));

        QVERIFY(populateOlderHistory(database));
        history.refresh();
        QTRY_COMPARE(history.rowCount(), 25);
        QVERIFY(history.hasMore());
        history.select(0);
        QTRY_COMPARE(history.savedAnswer().question(), QStringLiteral("History item 25"));
        QCOMPARE(history.selectedId(), QStringLiteral("9007199254741025"));
        QCOMPARE(history.savedAnswer().scope(),
                 (QStringList{"Repository 9007199254740993", "Branch: main",
                              "Actions run: 9007199254740994", "Attempt: 5"}));
        history.retranslate();
        history.deselect();
        QVERIFY(!history.savedAnswer().present());
        history.loadMore();
        QTRY_COMPARE(history.rowCount(), 27);
        QVERIFY(!history.hasMore());
        history.select(26);
        QTRY_COMPARE(history.savedAnswer().traceId(), persistedTrace);

        const auto oldToken = backend.session()->token;
        backend.restart();
        QTRY_VERIFY_WITH_TIMEOUT(backend.session() && backend.session()->token != oldToken, 10000);
        QTRY_COMPARE(history.rowCount(), 25);
        QVERIFY(!history.savedAnswer().present());
        QCOMPARE(answers.sessionQuestions().size(), 2);
        traced.inspect(persistedTrace, true);
        QTRY_VERIFY(traced.result().present());
        QVERIFY(!traced.live());
        QCOMPARE(traced.question(), QString(Question));
        QVERIFY(traced.details().sources().isEmpty());
        history.loadMore();
        QTRY_COMPARE(history.rowCount(), 27);
        history.select(26);
        QTRY_COMPARE(history.savedAnswer().traceId(), persistedTrace);

        preferences.setRetentionDays(1);
        QTRY_COMPARE(preferences.retentionDays(), 1);
        QTRY_COMPARE(history.rowCount(), 25);
        QVERIFY(!history.savedAnswer().present());
        QVERIFY(!traced.result().present());
        QVERIFY(makeNewestFixtureExpire(database));
        history.select(0);
        traced.inspect(ExpiringTrace, true);
        QTRY_VERIFY(history.savedAnswer().present());
        QTRY_COMPARE(traced.question(), QStringLiteral("History item 25"));
        QTRY_VERIFY_WITH_TIMEOUT(!history.savedAnswer().present(), 4000);
        QTRY_VERIFY_WITH_TIMEOUT(!traced.result().present(), 4000);
        QVERIFY(history.selectedId().isEmpty());
        QVERIFY(traced.question().isEmpty());

        traced.inspect(persistedTrace, true);
        QTRY_VERIFY(traced.result().present());
        const auto gatePath =
            directory.filePath("inference/Qwen3-4B-Instruct-2507-Q4_K_M.response-gate");
        QFile gate(gatePath);
        QVERIFY(gate.open(QIODevice::WriteOnly));
        gate.close();
        answers.setQuestion("Hold retry cooldown");
        answers.ask();
        const auto marker = directory.filePath("inference/Qwen3-4B-Instruct-2507-Q4_K_M.in-flight");
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(marker), 7000);
        preferences.clear();
        QTRY_VERIFY(!preferences.busy());
        QTRY_VERIFY(!history.loading());
        QCOMPARE(history.rowCount(), 0);
        QVERIFY(!history.savedAnswer().present());
        QVERIFY(answers.sessionQuestions().isEmpty());
        QVERIFY(!traced.result().present());
        QVERIFY(traced.question().isEmpty());
        traced.reload();
        QTRY_VERIFY(!traced.loading());
        QVERIFY(!traced.result().present());
        QVERIFY(gate.remove());
        QTRY_VERIFY_WITH_TIMEOUT(!answers.busy(), 7000);
        QVERIFY(answers.result().present());
        QVERIFY(answers.sessionQuestions().isEmpty());
        QTRY_VERIFY(!history.loading());
        QCOMPARE(history.rowCount(), 0);
        history.close();
    }

    void knowsWhichAnswersDependOnPurgedRepositories() {
        using ctxfab::AnswerResult;
        QCOMPARE(AnswerResult().affectedBy({77}), false);
        ctxfab::openapi::LocalAnswer answer;
        answer.setOutcome("answered");
        QVERIFY(AnswerResult(answer).affectedBy({77}));
        answer.setEvidenceRepositoryIds({9'007'199'254'740'993LL});
        QVERIFY(!AnswerResult(answer).affectedBy({77}));
        QVERIFY(!AnswerResult(answer).affectedBy({}));
        QVERIFY(AnswerResult(answer).affectedBy({9'007'199'254'740'993LL}));
        answer.setOutcome("insufficientEvidence");
        QVERIFY(!AnswerResult(answer).affectedBy({77}));
        answer.setEvidenceReview(ctxfab::openapi::AnswerEvidenceReview{});
        QVERIFY(AnswerResult(answer).affectedBy({77}));
    }

    void projectsTheExactModelSourcesAndEvidenceReview() {
        const ctxfab::openapi::LocalAnswer cited(QStringLiteral(R"({
            "outcome":"answered", "model":"fixture", "mode":"selected", "citations":[],
            "modelContext":{"question":"What changed?","sources":[
                {"slot":1,"kind":"history","repository":"acme/repo","branch":"main",
                "change":{"beforePath":"old.cpp","afterPath":"new.cpp","parent":"old-sha","child":"new-sha"},
                "parts":[
                    {"text":"old body","side":"before","path":"old.cpp","revision":"old-sha","startLine":7},
                    {"text":"new body","side":"after","path":"new.cpp","revision":"new-sha","startLine":8,"endLine":10}
                ]},
                {"slot":2,"kind":"current","parts":[{"text":"plain context"}]},
                {"slot":3,"kind":"workflow","parts":[{"text":"workflow context"}]}
            ]}
        })"));
        const ctxfab::TraceAnswerDetails sources(cited);
        QCOMPARE(sources.question(), QStringLiteral("What changed?"));
        QVERIFY(!sources.hasReview());
        QCOMPARE(sources.sources().size(), 3);
        const auto source = sources.sources().constFirst();
        QCOMPARE(source.title(), QStringLiteral("1. History"));
        QCOMPARE(source.scope(), QStringLiteral("acme/repo · main"));
        QCOMPARE(source.changes(),
                 (QStringList{"Before · old.cpp @ old-sha", "After · new.cpp @ new-sha"}));
        QCOMPARE(source.parts().at(0).location,
                 QStringLiteral("Before · old.cpp · @ old-sha · Line 7"));
        QCOMPARE(source.parts().at(1).location,
                 QStringLiteral("After · new.cpp · @ new-sha · Lines 8–10"));
        QCOMPARE(source.parts().at(1).text(), QStringLiteral("new body"));
        QCOMPARE(sources.sources().at(1).title(), QStringLiteral("2. Current code"));
        QCOMPARE(sources.sources().at(2).title(), QStringLiteral("3. GitHub Actions"));

        const ctxfab::openapi::LocalAnswer withheld(QStringLiteral(R"({
            "outcome":"insufficientEvidence", "model":"fixture", "mode":"bm25", "citations":[],
            "evidenceReview":{"question":"Why?", "scope":{"repositoryId":9007199254740993,"branch":"main"},
                "reason":"citationsUnverified","searches":[
                    {"source":"localPassages","candidateCount":1},
                    {"source":"githubHistory","candidateCount":0,"selection":"main at r1"},
                    {"source":"githubActions","candidateCount":0}
                ],"candidates":[{"source":"localPassages","reference":"new.cpp","revision":"new-sha",
                    "rank":2,"slot":1,"startLine":8,"endLine":10,"preview":"an excerpt",
                    "disposition":"citationUnverified"}]}
        })"));
        const ctxfab::TraceAnswerDetails review(withheld);
        QVERIFY(review.hasReview());
        QCOMPARE(review.question(), QStringLiteral("Why?"));
        QCOMPARE(review.reviewScope(), QStringLiteral("Repository 9007199254740993 · main"));
        QCOMPARE(review.searches().at(0).location, QStringLiteral("Keyword search (BM25)"));
        QCOMPARE(review.searches().at(1).location, QStringLiteral("main at r1"));
        QCOMPARE(review.searches().at(2).title, QStringLiteral("GitHub Actions attempts"));
        const auto candidate = review.candidates().constFirst();
        QCOMPARE(candidate.title, QStringLiteral("new.cpp"));
        QCOMPARE(
            candidate.location,
            QStringLiteral(
                "Local passage index · new-sha · Lines 8–10 · Search rank 2 · Proposed input 1"));
        QCOMPARE(candidate.text(), QStringLiteral("an excerpt"));
        QCOMPARE(
            candidate.status,
            QStringLiteral("Cited by the draft. Verification failed for the draft’s citations."));
    }

    void presentsImmutableCitationLocations() {
        ctxfab::openapi::Citation citation;
        ctxfab::openapi::SearchHit evidence;
        evidence.setSource("repo/main.cpp");
        evidence.setRevision("01234567890123456789");
        ctxfab::openapi::SourceLocation location;
        location.setPath("src/main.cpp");
        evidence.setLocation(location);
        citation.setEvidence(evidence);
        citation.setQuote("return 47;");
        citation.setLineUrl(
            "https://github.com/acme/repo/blob/01234567890123456789/src/main.cpp#L7-L8");

        ctxfab::openapi::HistoryCitation comparison;
        comparison.setSide("before");
        comparison.setChild("01234567890123456789");
        comparison.setParent("98765432109876543210");
        citation.setComparison(comparison);

        ctxfab::openapi::WorkflowRunCitation run;
        run.setRunId(9'007'199'254'740'993LL);
        run.setAttempt(2);
        run.setReportedHead("01234567890123456789");
        run.setRunUrl("https://github.com/acme/repo/actions/runs/9007199254740993/attempts/2");
        citation.setWorkflowRun(run);

        const ctxfab::AnswerCitation item(citation);
        QCOMPARE(item.label(), QStringLiteral("src/main.cpp#L7-L8 @ 0123456789"));
        QCOMPARE(item.quote(), QStringLiteral("return 47;"));
        QCOMPARE(item.comparison(), QStringLiteral("Before · 0123456789 vs 9876543210"));
        QCOMPARE(item.workflow(),
                 QStringLiteral("Run 9007199254740993 · attempt 2 · reported head 0123456789"));
        QCOMPARE(item.lines().fragment(), QStringLiteral("L7-L8"));
        QCOMPARE(item.run().host(), QStringLiteral("github.com"));

        citation.setLineUrl("https://github.com.example.test/acme/repo/blob/path#L7");
        run.setRunUrl("http://github.com/acme/repo/actions/runs/9007199254740993");
        citation.setWorkflowRun(run);
        const ctxfab::AnswerCitation external(citation);
        QVERIFY(external.lines().isEmpty());
        QVERIFY(external.run().isEmpty());
        QCOMPARE(external.label(), QStringLiteral("repo/main.cpp @ 01234567890123456789"));
    }

    void answersAndCancelsThroughTheLocalBackend() {
        const auto executable = qEnvironmentVariable("CTXFAB_TEST_BACKEND_EXECUTABLE");
        if (executable.isEmpty()) {
            QSKIP("Set CTXFAB_TEST_BACKEND_EXECUTABLE to exercise the Rust backend");
        }
        const QTemporaryDir profile;
        QVERIFY(profile.isValid());
        const QDir directory(profile.path());
        QVERIFY(provisionWorker(directory));
        QFile alternate(directory.filePath("inference/Qwen3-4B-Instruct-2507-Q5_K_M.gguf"));
        QVERIFY(alternate.open(QIODevice::WriteOnly));
        alternate.close();

        ctxfab::BackendSupervisor backend(
            {.executable = executable, .directory = profile.path(), .externalEndpoint = {}});
        ctxfab::Answers answers(backend);
        ctxfab::DiagnosticAnswerRequests requests(backend, answers);
        answers.refreshModels();
        QVERIFY(!answers.ready());
        backend.start();
        QTRY_VERIFY_WITH_TIMEOUT(answers.ready(), 8000);
        QTRY_VERIFY(!answers.activeModel().isEmpty());
        QVERIFY(seedCorpus(directory.filePath("context.sqlite3")));
        requests.feed()->setActive(true);
        QTRY_VERIFY(!requests.feed()->busy());
        QCOMPARE(requests.rowCount(), 0);
        const auto hasRecordedRequests = [&requests](int expected) {
            auto *feed = requests.feed();
            if (feed->rowCount() == expected) {
                return true;
            }
            if (!feed->busy()) {
                feed->refresh();
            }
            return false;
        };
        QVERIFY(!answers.canAsk());
        answers.setQuestion(QString(513, 'x'));
        QVERIFY(!answers.canAsk());
        answers.setQuestion(Question);
        QVERIFY(answers.canAsk());
        answers.ask();
        QVERIFY(answers.busy());
        QVERIFY(requests.pending());
        QVERIFY(requests.latestTraceId().isEmpty());
        QVERIFY(!answers.canAsk());
        QCOMPARE(answers.submittedQuestion(), QString(Question));
        QTRY_VERIFY_WITH_TIMEOUT(!answers.busy(), 7000);
        QVERIFY2(answers.result().present(), qPrintable(answers.feedback()));
        QVERIFY(answers.result().text().contains("47 seconds"));
        QCOMPARE(answers.result().citations().constFirst().label(),
                 QStringLiteral("src/outbound.rs @ r-native"));
        QCOMPARE(answers.traceId().size(), 32);
        const auto firstTrace = answers.traceId();
        QCOMPARE(requests.latestTraceId(), firstTrace);
        QCOMPARE(requests.latestQuestion(), QString(Question));
        QVERIFY(!requests.pending());
        QVERIFY(!requests.latestAbstained());
        QTRY_VERIFY(hasRecordedRequests(1));
        QCOMPARE(requests.rowCount(), 0);

        answers.setQuestion("orbitalzebramarker");
        answers.ask();
        QVERIFY(requests.pending());
        QCOMPARE(requests.rowCount(), 1);
        QTRY_VERIFY(!answers.busy());
        QVERIFY(answers.result().abstained());
        QVERIFY(answers.result().text().contains("zero candidates"));
        QVERIFY(answers.result().citations().isEmpty());
        QVERIFY(answers.traceId() != firstTrace);
        QCOMPARE(requests.latestTraceId(), answers.traceId());
        QCOMPARE(requests.latestQuestion(), QStringLiteral("orbitalzebramarker"));
        QVERIFY(requests.latestAbstained());
        QTRY_VERIFY(hasRecordedRequests(2));
        QCOMPARE(requests.rowCount(), 1);
        const auto earlier = requests.index(0, 0);
        const auto questionRole = requests.roleNames().key("question");
        const auto record =
            requests.data(earlier, ctxfab::DiagnosticLogs::SampleRole).value<ctxfab::TraceRecord>();
        QCOMPARE(record.traceId(), firstTrace);
        QCOMPARE(requests.data(earlier, questionRole).toString(), QString(Question));
        answers.repositoryPurged(77);
        QVERIFY(!answers.result().present());
        QVERIFY(answers.traceId().isEmpty());
        QVERIFY(answers.submittedQuestion().isEmpty());
        QVERIFY(requests.latestTraceId().isEmpty());
        QVERIFY(requests.latestQuestion().isEmpty());
        QCOMPARE(requests.rowCount(), 2);
        for (int row = 0; row < requests.rowCount(); ++row) {
            QVERIFY(requests.data(requests.index(row, 0), questionRole).toString().isEmpty());
        }
        answers.ask();
        QVERIFY(answers.busy());
        answers.repositoryPurged(77);
        QTRY_VERIFY(!answers.busy());
        QVERIFY(!answers.result().present());
        QVERIFY(answers.traceId().isEmpty());
        QCOMPARE(answers.feedback(),
                 QStringLiteral("Local repository data used by this answer was deleted."));

        answers.setQuestion("Hold retry cooldown");
        answers.ask();
        const auto inFlight =
            directory.filePath("inference/Qwen3-4B-Instruct-2507-Q4_K_M.in-flight");
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(inFlight), 7000);
        answers.cancelAnswer();
        QVERIFY(answers.cancelling());
        QTRY_VERIFY_WITH_TIMEOUT(!answers.busy(), 6000);
        QVERIFY(!answers.cancelling());
        QVERIFY(!answers.result().present());
        QCOMPARE(answers.feedback(), QStringLiteral("Answer request cancelled."));
        QCOMPARE(answers.traceId().size(), 32);
        QCOMPARE(requests.latestTraceId(), answers.traceId());
        QCOMPARE(requests.latestQuestion(), QStringLiteral("Hold retry cooldown"));
        QVERIFY(!requests.latestAbstained());

        answers.setQuestion(Question);
        answers.ask();
        QTRY_VERIFY_WITH_TIMEOUT(!answers.busy(), 7000);
        QVERIFY(answers.result().present());
        QVERIFY(answers.result().text().contains("47 seconds"));

        const auto original = answers.activeModel();
        QCOMPARE(answers.modelLabels().size(), 2);
        const auto other = 1 - answers.activeModelIndex();
        const auto session = backend.session();
        QVERIFY(session.has_value());
        const auto gatePath =
            directory.filePath("inference/Qwen3-4B-Instruct-2507-Q5_K_M.startup-gate");
        QFile gate(gatePath);
        QVERIFY(gate.open(QIODevice::WriteOnly));
        gate.close();
        answers.selectModel(other);
        QVERIFY(answers.modelsBusy());
        QVERIFY(!answers.canAsk());
        QTest::qWait(3200);
        QVERIFY(answers.modelsBusy());
        QCOMPARE(answers.activeModel(), original);
        QVERIFY(gate.remove());
        QTRY_VERIFY_WITH_TIMEOUT(!answers.modelsBusy(), 7000);
        QCOMPARE(answers.activeModelIndex(), other);
        QVERIFY(answers.activeModel() != original);
        QCOMPARE(backend.session()->url, session->url);
        QCOMPARE(backend.session()->token, session->token);
    }
};

QTEST_GUILESS_MAIN(AnswersTests)
#include "AnswersTests.moc"
