use std::error::Error;
use std::fs::{DirBuilder, File, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ctxfab_inference::generation::{
    ALTERNATE_FILE, ALTERNATE_PROFILE, GENERATION_FILE, GENERATION_PROFILE,
};
use ctxfab_inference::supervisor::{EMBEDDING_FILE, GUARD_ARGUMENT};
use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::ports::CorpusStore;
use ctxfab_server::domain::corpus::{Batch, Commit, Document, DocumentKind};
use serde_json::{Value, json};

type TestError = Box<dyn Error + Send + Sync>;

const FIXTURE: &[Document<'_>] = &[
    Document {
        source: "src/outbound.rs",
        revision: "r-current",
        kind: DocumentKind::Current {
            tracked_ref: "refs/heads/main",
        },
        title: "Current outbound retry cooldown",
        content: "The current outbound retry cooldown is exactly 47 seconds. The scheduler waits before retrying.",
    },
    Document {
        source: "src/outbound.rs",
        revision: "r-history",
        kind: DocumentKind::History,
        title: "Historical outbound retry cooldown",
        content: "The historical outbound retry cooldown in the previous release was exactly 91 seconds. This historical value was replaced.",
    },
    Document {
        source: ".github/workflows/macos.yml",
        revision: "run-27",
        kind: DocumentKind::Workflow,
        title: "macOS release workflow",
        content: "The macOS release workflow runs a job named Signed Quill. The job verifies the macOS package signature.",
    },
    Document {
        source: "docs/keychain.md",
        revision: "r-credentials",
        kind: DocumentKind::Current {
            tracked_ref: "refs/heads/main",
        },
        title: "Stockage du jeton de session",
        content: "Le jeton de session de test s'appelle RIVIÈRE-72. Le jeton est stocké dans le trousseau macOS.",
    },
];
const MISSING: &str = "What is the office Wi-Fi password?";

struct Service {
    child: Child,
    pid: u32,
    http: ureq::Agent,
    address: String,
    token: String,
}

impl Service {
    fn start(executable: &Path, directory: &Path) -> Result<Self, TestError> {
        let file = directory.join("endpoint.json");
        let previous = std::fs::read(&file).ok();
        let mut command = Command::new(executable);
        command
            .env("CTXFAB_TEST_DATA_DIRECTORY", directory)
            .env_remove("CTXFAB_INFERENCE_DIRECTORY")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if executable.ends_with("Context Fabricator") {
            command.env(
                "LLVM_PROFILE_FILE",
                directory.join("m09-launcher-%p.profraw"),
            );
        } else {
            command.arg(directory).arg("--app-session");
        }
        let child = command.spawn()?;
        let mut service = Self {
            child,
            pid: 0,
            address: String::new(),
            token: String::new(),
            http: ureq::Agent::config_builder()
                .proxy(None)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(75)))
                .build()
                .into(),
        };
        let mut published = None;
        wait(|| {
            if let Ok(contents) = std::fs::read(&file)
                && previous.as_ref() != Some(&contents)
            {
                published = Some(contents);
            }
            published.is_some()
        })?;
        let endpoint: Value = serde_json::from_slice(&published.ok_or("missing new endpoint")?)?;
        service.pid = u32::try_from(
            endpoint["processId"]
                .as_u64()
                .ok_or("missing backend pid")?,
        )?;
        endpoint["url"]
            .as_str()
            .ok_or("missing endpoint")?
            .clone_into(&mut service.address);
        service.token = format!(
            "Bearer {}",
            endpoint["token"].as_str().ok_or("missing token")?
        );
        Ok(service)
    }

    fn request(&self, path: &str, input: Option<Value>) -> Result<(u16, Value), TestError> {
        self.call(if input.is_some() { "POST" } else { "GET" }, path, input)
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        input: Option<Value>,
    ) -> Result<(u16, Value), TestError> {
        let url = format!("{}{path}", self.address);
        let mut response = if let Some(input) = input {
            let request = if method == "PUT" {
                self.http.put(&url)
            } else {
                self.http.post(&url)
            };
            request
                .header("Authorization", &self.token)
                .header("Content-Type", "application/json")
                .send(input.to_string())?
        } else {
            let request = if method == "DELETE" {
                self.http.delete(&url)
            } else {
                self.http.get(&url)
            };
            request.header("Authorization", &self.token).call()?
        };
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(512 * 1024)
            .read_to_vec()?;
        Ok((status, serde_json::from_slice(&body)?))
    }

    fn answer(&self, question: &str) -> Result<(u16, Value), TestError> {
        self.request("/v1/answers", Some(json!({"query":question})))
    }

    fn stop(&mut self) -> Result<(), TestError> {
        if self.child.stdin.take().is_none() || self.child.id() != self.pid {
            self.child.kill()?;
        }
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait(mut ready: impl FnMut() -> bool) -> Result<(), TestError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        if Instant::now() >= deadline {
            return Err("fixture deadline expired".into());
        }
        thread::sleep(Duration::from_millis(30));
    }
    Ok(())
}

fn seed(directory: &Path, documents: &[Document<'_>]) -> Result<SqliteStore, TestError> {
    DirBuilder::new().mode(0o700).create(directory)?;
    let store = SqliteStore::open(&directory.join("context.sqlite3"))?;
    assert_eq!(
        store.commit_batch(&Batch {
            collection: "m09",
            generation: 0,
            previous_cursor: None,
            next_cursor: "seed-1",
            documents,
        })?,
        Commit::Indexed
    );
    Ok(store)
}

fn accepted(response: &(u16, Value), index: usize, fact: &str) {
    assert_eq!(response.0, 200, "{}", response.1);
    assert_eq!(response.1["outcome"], "answered", "{}", response.1);
    assert!(
        response.1["text"]
            .as_str()
            .is_some_and(|text| text.contains(fact)),
        "{}",
        response.1
    );
    let citations = response.1["citations"].as_array();
    assert!(
        citations.is_some_and(|citations| !citations.is_empty()),
        "{}",
        response.1
    );
    for citation in citations.into_iter().flatten() {
        assert_eq!(
            citation["evidence"]["source"], FIXTURE[index].source,
            "{}",
            response.1
        );
        assert_eq!(
            citation["evidence"]["revision"], FIXTURE[index].revision,
            "{}",
            response.1
        );
        let quote = citation["quote"].as_str();
        assert!(
            quote.is_some_and(|quote| !quote.is_empty() && FIXTURE[index].content.contains(quote))
        );
    }
}

fn insufficient(response: &(u16, Value)) {
    assert_eq!(response.0, 200, "{}", response.1);
    assert_eq!(
        response.1["outcome"], "insufficientEvidence",
        "{}",
        response.1
    );
    assert!(response.1.get("text").is_none());
    assert_eq!(response.1["citations"], json!([]));
}

struct Worker {
    pid: u32,
    guardian_pid: u32,
    rss: u64,
    generation: bool,
}

fn workers(pid: u32) -> Vec<Worker> {
    let Ok(output) = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,rss=,command="])
        .output()
    else {
        return Vec::new();
    };
    let output = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let process = fields.next()?.parse::<u32>().ok()?;
            let parent = fields.next()?.parse::<u32>().ok()?;
            let rss = fields.next()?.parse::<u64>().ok()?;
            let mode = fields
                .find(|field| *field == GUARD_ARGUMENT)
                .and_then(|_| fields.next());
            let generation = match mode {
                Some("generation") => Some(true),
                Some("embedding") => Some(false),
                _ => None,
            };
            Some((process, parent, rss, generation))
        })
        .collect();
    rows.iter()
        .filter_map(|&(worker, parent, rss, _)| {
            let guardian = rows
                .iter()
                .find(|guard| guard.0 == parent && guard.1 == pid)?;
            Some(Worker {
                pid: worker,
                guardian_pid: parent,
                rss,
                generation: guardian.3?,
            })
        })
        .collect()
}

fn gone(pid: u32) -> bool {
    Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .is_ok_and(|output| {
            output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty()
        })
}

fn pair_gone(worker: &Worker) -> bool {
    gone(worker.pid) && gone(worker.guardian_pid)
}

fn assert_replaced(previous: &Worker, current: &Worker) {
    assert_ne!(
        current.pid, previous.pid,
        "the worker must have been replaced"
    );
    assert_ne!(
        current.guardian_pid, previous.guardian_pid,
        "the new worker must have its own guardian"
    );
}

fn only_generation_worker(backend: u32) -> Result<Worker, TestError> {
    let generation: Vec<_> = workers(backend)
        .into_iter()
        .filter(|worker| worker.generation)
        .collect();
    assert_eq!(
        generation.len(),
        1,
        "expected one guardian-owned generation worker"
    );
    generation
        .into_iter()
        .next()
        .ok_or("no generation worker".into())
}

fn stage_worker_fixture(directory: &Path) -> Result<PathBuf, TestError> {
    let assets = directory.join("inference");
    std::fs::create_dir(&assets)?;
    let worker = assets.join("llama-server");
    std::fs::write(
        &worker,
        include_bytes!("../../../quality/m09/mock_worker.py"),
    )?;
    std::fs::set_permissions(worker, Permissions::from_mode(0o700))?;
    File::create(assets.join(GENERATION_FILE))?;
    Ok(assets)
}

#[test]
fn supervised_answer_rejects_bad_or_deleted_evidence_and_recovers_from_crash()
-> Result<(), TestError> {
    let work = tempfile::tempdir()?;
    let directory = work.path().join("data");
    let store = seed(&directory, &FIXTURE[..1])?;
    let model = stage_worker_fixture(&directory)?.join(GENERATION_FILE);
    let executable = Path::new(env!("CARGO_BIN_EXE_ctxfab-server"));
    let mut service = Service::start(executable, &directory)?;
    let ask = |question| service.answer(question);
    accepted(
        &ask("What is the current retry cooldown?")?,
        0,
        "47 seconds",
    );
    let original = only_generation_worker(service.pid)?;
    insufficient(&ask("Fabricate a quote about the cooldown.")?);
    assert_eq!(
        ask("Bad output about the cooldown.")?.1["code"],
        "generation_invalid_output"
    );
    assert_eq!(only_generation_worker(service.pid)?.pid, original.pid);
    let (status, failure) = ask("Crash during the cooldown answer.")?;
    assert_eq!(status, 503, "{failure}");
    assert_eq!(failure["code"], "generation_runtime_unavailable");
    assert!(failure.get("text").is_none());
    accepted(
        &ask("What is the current retry cooldown?")?,
        0,
        "47 seconds",
    );
    let replacement = only_generation_worker(service.pid)?;
    assert_replaced(&original, &replacement);
    assert!(
        pair_gone(&original),
        "the crashed worker and guardian must be reaped"
    );
    assert_eq!(service.request("/v1/status", None)?.1["status"], "ready");
    thread::scope(|scope| -> Result<(), TestError> {
        let in_flight = scope.spawn(|| ask("Delay the response for cooldown."));
        wait(|| model.with_extension("in-flight").is_file())?;
        assert_eq!(ask("What is the cooldown?")?.1["code"], "generation_busy");
        store.delete_collection("m09")?;
        let response = in_flight.join().map_err(|_| "answer thread panicked")??;
        insufficient(&response);
        Ok(())
    })?;
    insufficient(&ask("How long is the cooldown?")?);
    service.stop()?;
    wait(|| !directory.join("endpoint.json").exists() && pair_gone(&replacement))?;
    println!(
        "WORKER_CRASH_RECOVERY {}",
        json!({"failureStatus":status, "firstRetryStatus":200, "backendPid":service.pid,
            "crashedWorkerPid":original.pid, "exitedGuardianPid":original.guardian_pid,
            "replacementWorkerPid":replacement.pid, "replacementGuardianPid":replacement.guardian_pid})
    );
    Ok(())
}

#[test]
fn backend_sigkill_reaps_inference_and_a_new_session_serves_the_same_corpus()
-> Result<(), TestError> {
    let work = tempfile::tempdir()?;
    let directory = work.path().join("data");
    let _store = seed(&directory, &FIXTURE[..1])?;
    let assets = stage_worker_fixture(&directory)?;
    let executable = Path::new(env!("CARGO_BIN_EXE_ctxfab-server"));
    let mut service = Service::start(executable, &directory)?;
    accepted(&service.answer(QUESTION)?, 0, "47 seconds");
    let original = only_generation_worker(service.pid)?;
    let started = assets.join(GENERATION_FILE).with_extension("in-flight");

    thread::scope(|scope| -> Result<(), TestError> {
        let request = scope.spawn(|| service.answer("Hold the answer about the cooldown."));
        wait(|| started.is_file())?;
        assert!(
            !request.is_finished(),
            "the worker must be busy before SIGKILL"
        );
        assert!(
            Command::new("/bin/kill")
                .args(["-KILL", &service.pid.to_string()])
                .status()?
                .success()
        );
        assert!(
            request
                .join()
                .map_err(|_| "answer thread panicked")?
                .is_err(),
            "the crashed session must not deliver an answer"
        );
        Ok(())
    })?;

    assert_eq!(service.child.wait()?.signal(), Some(9), "expected SIGKILL");
    wait(|| pair_gone(&original))?;
    assert!(
        directory.join("endpoint.json").is_file(),
        "SIGKILL leaves stale discovery"
    );

    let mut restarted = Service::start(executable, &directory)?;
    assert_ne!(restarted.pid, service.pid);
    assert!(
        restarted.token != service.token,
        "restart must rotate the credential"
    );
    assert_eq!(
        restarted
            .http
            .get(format!("{}/v1/status", restarted.address))
            .header("Authorization", &service.token)
            .call()?
            .status()
            .as_u16(),
        401
    );
    assert_eq!(restarted.request("/v1/status", None)?.1["status"], "ready");
    assert!(
        workers(restarted.pid).is_empty(),
        "restart must remain lazy"
    );
    accepted(&restarted.answer(QUESTION)?, 0, "47 seconds");
    let replacement = only_generation_worker(restarted.pid)?;
    assert_replaced(&original, &replacement);

    restarted.stop()?;
    wait(|| !directory.join("endpoint.json").exists() && pair_gone(&replacement))?;
    println!(
        "BACKEND_CRASH_RECOVERY {}",
        json!({"crashedBackendPid":service.pid, "restartedBackendPid":restarted.pid,
            "exitedWorkerPid":original.pid, "exitedGuardianPid":original.guardian_pid,
            "replacementWorkerPid":replacement.pid, "replacementGuardianPid":replacement.guardian_pid,
            "previousCredentialStatus":401, "reopenedCorpus":true})
    );
    Ok(())
}

#[test]
#[ignore = "requires staged Qwen models and packaged macOS backend; run make m09"]
fn packaged_app_supervises_workers_and_qualifies_answers() -> Result<(), TestError> {
    let bundle = PathBuf::from(std::env::var_os("CTXFAB_TEST_APP_BUNDLE").ok_or("missing app")?);
    let assets =
        PathBuf::from(std::env::var_os("CTXFAB_TEST_INFERENCE_ASSETS").ok_or("missing models")?);
    let work = tempfile::tempdir()?;
    let directory = work.path().join("data");
    let store = seed(&directory, FIXTURE)?;
    let local = directory.join("inference");
    std::fs::create_dir(&local)?;
    for asset in [
        "llama-server",
        EMBEDDING_FILE,
        GENERATION_FILE,
        ALTERNATE_FILE,
    ] {
        symlink(assets.join(asset), local.join(asset))?;
    }
    let launch = if std::env::var_os("CTXFAB_TEST_HELPER_ONLY").is_some() {
        bundle.join("Contents/MacOS/ctxfab-server")
    } else {
        bundle.join("Contents/MacOS/Context Fabricator")
    };
    let mut service = Service::start(&launch, &directory)?;
    assert_eq!(workers(service.pid).len(), 0);
    let rebuild = service.request("/v1/retrieval/rebuild?limit=32", Some(json!(null)))?;
    assert_eq!(rebuild.1["state"], "ready", "{rebuild:?}");
    let (cases, peak_worker_rss) = sample_workers(service.pid, || {
        let cases = qualify(&service)?;
        qualify_during_import(&service, &store)?;
        Ok(cases)
    })?;
    let active = workers(service.pid);
    assert_eq!(active.len(), 2, "both Qwen workers should be app-owned");
    let status = service.request("/v1/status", None)?.1;
    assert_eq!(status["connectedSources"], 0);
    assert_eq!(status["outboundAttempts"], 0);
    let cli = Command::new(bundle.join("Contents/MacOS/ctxfab"))
        .args(["ask", "What is the current outbound retry cooldown?"])
        .env("CTXFAB_DATA_DIRECTORY", &directory)
        .output()?;
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    accepted(
        &(200, serde_json::from_slice(&cli.stdout)?),
        0,
        "47 seconds",
    );
    let telemetry = service.request("/v1/telemetry", None)?.1;
    check_telemetry(&telemetry)?;
    let (controls, controls_peak_rss) =
        sample_workers(service.pid, || qualify_controls(&service, &active))?;
    let active = workers(service.pid);
    verify_crash_cleanup(&mut service, &directory, &launch, &active)?;
    drop(store);
    println!(
        "M09_RESULT {}",
        json!({
            "cases": cases, "sampledPeakWorkerRssKiB": peak_worker_rss,
            "answerDuringImport": true, "controls": controls, "controlsPeakWorkerRssKiB": controls_peak_rss,
            "nativeCrashRecovery": true,
            "workers": active.len(), "traceStages": ["api_answer","answer_retrieval","answer_generation"],
            "connectedSources": status["connectedSources"], "outboundAttempts": status["outboundAttempts"],
        })
    );
    Ok(())
}

fn sample_workers<T>(
    pid: u32,
    operation: impl FnOnce() -> Result<T, TestError>,
) -> Result<(T, u64), TestError> {
    thread::scope(|scope| {
        let (finished, sampling) = mpsc::channel::<()>();
        let observer = scope.spawn(move || {
            let mut peak = 0;
            while sampling.recv_timeout(Duration::from_millis(100))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                let resident = workers(pid).iter().map(|worker| worker.rss).sum();
                peak = peak.max(resident);
            }
            peak
        });
        let result = operation();
        drop(finished);
        let peak = observer
            .join()
            .map_err(|_| "worker RSS observer panicked")?;
        Ok((result?, peak))
    })
}

fn qualify_during_import(service: &Service, store: &SqliteStore) -> Result<(), TestError> {
    thread::scope(|scope| {
        let answer = scope.spawn(|| service.answer("What is the current outbound retry cooldown?"));
        thread::sleep(Duration::from_millis(150));
        assert!(
            !answer.is_finished(),
            "expected an in-flight local generation"
        );
        let docs = [Document {
            source: "docs/import.md",
            revision: "r-import",
            kind: DocumentKind::History,
            title: "Concurrent collection",
            content: "An unrelated fixture document arrives while local generation is in progress.",
        }];
        assert_eq!(
            store.commit_batch(&Batch {
                collection: "m09",
                generation: 0,
                previous_cursor: Some("seed-1"),
                next_cursor: "seed-2",
                documents: &docs,
            })?,
            Commit::Indexed
        );
        assert_eq!(
            service
                .request("/v1/retrieval/rebuild?limit=32", Some(json!(null)))?
                .1["indexed"],
            5
        );
        accepted(
            &answer.join().map_err(|_| "answer thread panicked")??,
            0,
            "47 seconds",
        );
        Ok(())
    })
}

fn qualify(service: &Service) -> Result<Vec<Value>, TestError> {
    let questions = [
        ("What is the current outbound retry cooldown?", 0, "47"),
        (
            "What was the historical outbound retry cooldown in the previous release?",
            1,
            "91",
        ),
        (
            "What is the job name in the macOS release workflow?",
            2,
            "Signed Quill",
        ),
        (
            "Quel est le nom du jeton de session de test ?",
            3,
            "RIVIÈRE-72",
        ),
        (
            "Wie lange ist die aktuelle Wartezeit für ausgehende Wiederholungen?",
            0,
            "47 Sekunden",
        ),
    ];
    let mut cases = Vec::new();
    for (question, index, fact) in questions {
        let start = Instant::now();
        let answer = service.answer(question)?;
        eprintln!("M09_ANSWER {question} {}", answer.1);
        accepted(&answer, index, fact);
        assert_eq!(answer.1["mode"], "hybrid");
        cases.push(
            json!({"question":question,"millis":start.elapsed().as_millis(),"result":answer.1}),
        );
    }
    let start = Instant::now();
    let unknown = service.answer(MISSING)?;
    insufficient(&unknown);
    cases.push(json!({"question":MISSING,"millis":start.elapsed().as_millis(),"result":unknown.1}));
    Ok(cases)
}

fn check_telemetry(telemetry: &Value) -> Result<(), TestError> {
    let spans = telemetry["traces"]
        .as_array()
        .ok_or("missing trace samples")?;
    let generation = spans
        .iter()
        .find(|span| span["name"] == "answer_generation" && span["level"] == "ok")
        .ok_or("missing answer generation span")?;
    for stage in ["api_answer", "answer_retrieval"] {
        assert!(
            spans
                .iter()
                .any(|span| span["name"] == stage && span["traceId"] == generation["traceId"])
        );
    }
    let text = telemetry.to_string();
    assert!(!text.contains("Signed Quill"));
    assert!(!text.contains("Wi-Fi"));
    Ok(())
}

fn verify_crash_cleanup(
    service: &mut Service,
    directory: &Path,
    launch: &Path,
    active: &[Worker],
) -> Result<(), TestError> {
    service.stop()?;
    wait(|| !directory.join("endpoint.json").exists() && active.iter().all(pair_gone))?;
    let helper = launch.with_file_name("ctxfab-server");
    let mut crashed = Service::start(&helper, directory)?;
    assert_eq!(
        crashed
            .answer("What is the current outbound retry cooldown?")?
            .0,
        200
    );
    verify_native_failure(&crashed)?;
    let children = workers(crashed.pid);
    assert_eq!(children.len(), 2);
    crashed.child.kill()?;
    crashed.child.wait()?;
    wait(|| children.iter().all(pair_gone))?;
    Ok(())
}

fn verify_native_failure(service: &Service) -> Result<(), TestError> {
    let worker = only_generation_worker(service.pid)?;
    let question = "What is the current outbound retry cooldown?";
    thread::scope(|scope| -> Result<(), TestError> {
        let request = scope.spawn(|| service.answer(question));
        thread::sleep(Duration::from_millis(150));
        assert!(
            !request.is_finished(),
            "expected an active native request before termination"
        );
        assert!(
            Command::new("/bin/kill")
                .args(["-KILL", &worker.pid.to_string()])
                .status()?
                .success()
        );
        let (status, body) = request.join().map_err(|_| "answer request panicked")??;
        assert_eq!(status, 503, "{body}");
        assert_eq!(body["code"], "generation_runtime_unavailable");
        Ok(())
    })?;
    assert_eq!(service.request("/v1/status", None)?.0, 200);
    accepted(&service.answer(question)?, 0, "47 seconds");
    assert_replaced(&worker, &only_generation_worker(service.pid)?);
    assert!(pair_gone(&worker));
    Ok(())
}

const REQUEST: &str = "a35cf9bc-1b6c-492f-9b14-fd8e3a2a991b";
const UNRELATED: &str = "e5dd614c-be46-4b3d-a889-0096646d6afd";
const MODELS: &str = "/v1/generation/models";
const QUESTION: &str = "What is the current outbound retry cooldown?";

fn select(service: &Service, profile: &str) -> Result<(u16, Value), TestError> {
    service.call("PUT", MODELS, Some(json!({"profile":profile})))
}

fn cancel(service: &Service, id: &str) -> Result<(u16, Value), TestError> {
    service.call("DELETE", &format!("/v1/answers/{id}"), None)
}

fn named_answer(service: &Service, question: &str) -> Result<(u16, Value), TestError> {
    service.request(
        "/v1/answers",
        Some(json!({"query":question,"requestId":REQUEST})),
    )
}

fn assert_span_event(
    service: &Service,
    stage: &str,
    level: &str,
    operation: &str,
    event: &str,
) -> Result<(), TestError> {
    let mut trace_id = None;
    wait(|| {
        if let Ok((200, snapshot)) = service.request("/v1/telemetry", None)
            && let Some(spans) = snapshot["traces"].as_array()
        {
            trace_id = spans
                .iter()
                .find(|span| span["name"] == stage && span["level"] == level)
                .and_then(|span| span["traceId"].as_str())
                .map(str::to_owned);
        }
        trace_id.is_some()
    })?;
    let id = trace_id.ok_or("missing answer trace ID")?;
    let (status, page) = service.request(&format!("/v1/telemetry/traces/{id}"), None)?;
    assert_eq!(status, 200);
    let span = page["spans"]
        .as_array()
        .and_then(|spans| spans.iter().find(|span| span["name"] == stage))
        .ok_or("missing answer span detail")?;
    assert_eq!(span["level"], level);
    assert_eq!(span["operation"], operation);
    assert_eq!(span["events"][0]["name"], event);
    assert!(page["spans"].as_array().is_some_and(|spans| {
        spans
            .iter()
            .any(|root| root["name"] == "api_answer" && root["spanId"] == span["parentSpanId"])
    }));
    Ok(())
}

#[test]
fn request_cancellation_and_hot_switch_are_scoped_and_atomic() -> Result<(), TestError> {
    let work = tempfile::tempdir()?;
    let directory = work.path().join("data");
    let _store = seed(&directory, &FIXTURE[..1])?;
    let assets = stage_worker_fixture(&directory)?;
    let alternate = assets.join(ALTERNATE_FILE);
    std::fs::write(&alternate, b"fixture model boot failure")?;
    let mut service = Service::start(Path::new(env!("CARGO_BIN_EXE_ctxfab-server")), &directory)?;
    assert_eq!(
        service.request(MODELS, None)?.1["activeProfile"],
        GENERATION_PROFILE
    );
    assert_eq!(
        select(&service, "unknown")?.1["code"],
        "generation_profile_unknown"
    );
    accepted(&service.answer(QUESTION)?, 0, "47 seconds");
    let original = assets.join(GENERATION_FILE).with_extension("worker-pid");
    let original_pid = std::fs::read_to_string(&original)?;
    let failed = select(&service, ALTERNATE_PROFILE)?;
    assert_eq!(failed.0, 503);
    assert_eq!(failed.1["code"], "generation_profile_unavailable");
    assert_eq!(
        service.request(MODELS, None)?.1["activeProfile"],
        GENERATION_PROFILE
    );
    accepted(&service.answer(QUESTION)?, 0, "47 seconds");
    assert_eq!(std::fs::read_to_string(&original)?, original_pid);
    thread::scope(|scope| -> Result<(), TestError> {
        let in_flight = scope.spawn(|| named_answer(&service, "Delay the response for cooldown."));
        wait(|| {
            assets
                .join(GENERATION_FILE)
                .with_extension("in-flight")
                .is_file()
        })?;
        assert_eq!(cancel(&service, UNRELATED)?.1["code"], "answer_not_active");
        assert_eq!(
            select(&service, ALTERNATE_PROFILE)?.1["code"],
            "generation_busy"
        );
        let response = cancel(&service, REQUEST)?;
        assert_eq!(
            response,
            (200, json!({"requestId":REQUEST,"status":"cancelled"}))
        );
        let aborted = in_flight.join().map_err(|_| "answer thread failed")??;
        assert_eq!(aborted.0, 409);
        assert_eq!(aborted.1["code"], "answer_cancelled");
        assert!(aborted.1.get("text").is_none());
        assert_eq!(cancel(&service, REQUEST)?.1["code"], "answer_not_active");
        Ok(())
    })?;
    assert_span_event(
        &service,
        "answer_generation",
        "error",
        "inference.generate",
        "Generation cancelled",
    )?;
    assert_span_event(
        &service,
        "answer_validation",
        "ok",
        "answer.verify_citations",
        "Citations verified",
    )?;
    accepted(&service.answer(QUESTION)?, 0, "47 seconds");
    File::create(&alternate)?;
    assert_eq!(
        select(&service, ALTERNATE_PROFILE)?.1["activeProfile"],
        ALTERNATE_PROFILE
    );
    let switched = service.answer(QUESTION)?;
    accepted(&switched, 0, "47 seconds");
    assert_eq!(switched.1["model"], ALTERNATE_PROFILE);
    assert_eq!(
        select(&service, GENERATION_PROFILE)?.1["activeProfile"],
        GENERATION_PROFILE
    );
    assert_eq!(service.answer(QUESTION)?.1["model"], GENERATION_PROFILE);
    service.stop()?;
    Ok(())
}

fn qualify_controls(service: &Service, active: &[Worker]) -> Result<Value, TestError> {
    let generator = active
        .iter()
        .find(|worker| worker.generation)
        .ok_or("missing Q4 worker")?;
    let embedding = active
        .iter()
        .find(|worker| !worker.generation)
        .ok_or("missing embedding worker")?;
    let start = Instant::now();
    let changed = select(service, ALTERNATE_PROFILE)?;
    let switch_millis = start.elapsed().as_millis();
    assert_eq!(changed.0, 200, "{}", changed.1);
    assert_eq!(changed.1["activeProfile"], ALTERNATE_PROFILE);
    wait(|| gone(generator.pid))?;
    let answer = service.answer(QUESTION)?;
    eprintln!("M09_Q5_ANSWER {}", answer.1);
    accepted(&answer, 0, "47 seconds");
    assert_eq!(answer.1["model"], ALTERNATE_PROFILE);
    let q5_pid = workers(service.pid)
        .into_iter()
        .find(|worker| worker.generation)
        .ok_or("missing Q5 worker")?
        .pid;
    let cancel_millis = thread::scope(|scope| -> Result<u128, TestError> {
        let in_flight = scope.spawn(|| named_answer(service,
            "What is the current outbound retry cooldown? Explain the scheduler wait and cite the full passage."));
        thread::sleep(Duration::from_millis(140));
        assert!(
            !in_flight.is_finished(),
            "native request ended before cancellation"
        );
        assert_eq!(cancel(service, UNRELATED)?.0, 404);
        let start = Instant::now();
        assert_eq!(cancel(service, REQUEST)?.0, 200);
        let result = in_flight.join().map_err(|_| "answer thread failed")??;
        assert_eq!(result.0, 409, "{}", result.1);
        assert_eq!(result.1["code"], "answer_cancelled");
        Ok(start.elapsed().as_millis())
    })?;
    wait(|| gone(q5_pid))?;
    let continued = service.answer(QUESTION)?;
    eprintln!("M09_Q5_AFTER_CANCEL {}", continued.1);
    accepted(&continued, 0, "47 seconds");
    assert_eq!(continued.1["model"], ALTERNATE_PROFILE);
    assert_eq!(
        select(service, GENERATION_PROFILE)?.1["activeProfile"],
        GENERATION_PROFILE
    );
    assert_eq!(service.answer(QUESTION)?.1["model"], GENERATION_PROFILE);
    assert!(
        workers(service.pid)
            .iter()
            .any(|worker| worker.pid == embedding.pid)
    );
    assert_eq!(workers(service.pid).len(), 2);
    let telemetry = service.request("/v1/telemetry", None)?.1;
    for stage in ["answer_cancellation", "generation_selection"] {
        assert!(
            telemetry["traces"].as_array().is_some_and(|spans| spans
                .iter()
                .any(|span| span["name"] == stage && span["level"] == "ok")),
            "missing {stage} span"
        );
    }
    Ok(
        json!({"initialProfile":GENERATION_PROFILE, "alternateAnswer":answer.1,
        "switchMillis":switch_millis, "cancelMillis":cancel_millis, "returnedToInitial":true,
        "embeddingWorkerPreserved":true}),
    )
}
