//! Bounded local generation with schema-constrained output.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::supervisor::{Client, ManagedWorker, Profile, Runtime, WorkerError};

mod budget;
mod policy;
mod timing;
pub use budget::ContextBudget;
pub use policy::{ComparisonDraft, ComparisonEvidence, Draft, MAX_PASSAGE_CITATIONS, Reference};
use policy::{
    LinkedDraft, SEARCH_PROMPT, SearchFormulation, answer_prompt, comparison_prompt,
    comparison_schema, linked_evidence_prompt, quote_schema, search_schema,
};
pub use timing::GenerationTiming;

pub const GENERATION_PROFILE: &str = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1";
pub const GENERATION_SHA256: &str =
    "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597";
pub const ALTERNATE_SHA256: &str =
    "5bde5e9d883622acb02bf77fe7dcbc56a8b9a9ad4be78a72ca23a532658b4ecb";
pub const GENERATION_FILE: &str = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
pub const GENERATION_LABEL: &str = "Qwen3 4B · Q4_K_M";
pub const ALTERNATE_PROFILE: &str = "qwen3-4b-instruct-2507-q5-k-m-5bde5e9d-v1";
pub const ALTERNATE_FILE: &str = "Qwen3-4B-Instruct-2507-Q5_K_M.gguf";
pub const ALTERNATE_LABEL: &str = "Qwen3 4B · Q5_K_M";

#[derive(Clone, Serialize)]
pub struct Model {
    pub id: Cow<'static, str>,
    pub label: Cow<'static, str>,
    #[serde(skip)]
    pub(crate) file: Cow<'static, str>,
    #[serde(skip)]
    fingerprint: Cow<'static, str>,
}

const MODELS: [Model; 2] = [
    Model {
        id: Cow::Borrowed(GENERATION_PROFILE),
        label: Cow::Borrowed(GENERATION_LABEL),
        file: Cow::Borrowed(GENERATION_FILE),
        fingerprint: Cow::Borrowed(GENERATION_SHA256),
    },
    Model {
        id: Cow::Borrowed(ALTERNATE_PROFILE),
        label: Cow::Borrowed(ALTERNATE_LABEL),
        file: Cow::Borrowed(ALTERNATE_FILE),
        fingerprint: Cow::Borrowed(ALTERNATE_SHA256),
    },
];

impl Model {
    /// Resolve only a known curated or verified imported profile.
    #[must_use]
    pub fn managed(id: &str, label: &str, fingerprint: &str) -> Option<Self> {
        MODELS
            .iter()
            .find(|model| model.id == id && model.fingerprint == fingerprint)
            .cloned()
            .or_else(|| Self::imported(id.to_owned(), label.to_owned(), fingerprint.to_owned()))
    }

    /// A verified imported profile always resolves to its own managed local filename.
    #[must_use]
    pub fn imported(id: String, label: String, fingerprint: String) -> Option<Self> {
        (valid_imported_alias(&id) && valid_fingerprint(&fingerprint)).then(|| Self {
            file: format!("{id}.gguf").into(),
            id: id.into(),
            label: label.into(),
            fingerprint: fingerprint.into(),
        })
    }
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Persistence for an explicitly confirmed model identity and its weight fingerprint.
pub trait SelectionStore: Send + Sync {
    /// # Errors
    /// Returns an error if selection state cannot be read.
    fn load(&self) -> Result<Option<(String, String)>, SelectionStoreError>;
    /// # Errors
    /// Returns an error if the selection cannot be committed.
    fn save(&self, profile: &str, fingerprint: &str) -> Result<(), SelectionStoreError>;
}

#[derive(Debug)]
pub struct SelectionStoreError;

#[must_use]
pub fn valid_generation_alias(alias: &str) -> bool {
    matches!(alias, GENERATION_PROFILE | ALTERNATE_PROFILE) || valid_imported_alias(alias)
}

fn valid_imported_alias(alias: &str) -> bool {
    alias.strip_prefix("hf-").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GenerationError {
    #[error("The local generation worker is unavailable.")]
    Runtime,
    #[error("The local generation worker is busy.")]
    Busy,
    #[error("The local generation deadline expired.")]
    Deadline,
    #[error("The local generation worker returned invalid output.")]
    Output,
    #[error("The answer request was cancelled.")]
    Cancelled,
    #[error("There is no active answer with this request ID.")]
    UnknownRequest,
    #[error("This generation profile is not provisioned locally.")]
    UnknownProfile,
    #[error("The requested generation profile could not start. The previous selection was kept.")]
    ProfileUnavailable,
}

impl From<WorkerError> for GenerationError {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::Busy => Self::Busy,
            WorkerError::Deadline => Self::Deadline,
            WorkerError::Unavailable => Self::Runtime,
        }
    }
}

/// Choose the source layouts that may be measured for an ordinary answer.
#[derive(Clone, Copy)]
pub enum PassagePacking {
    Compact(usize),
    RankPrefixFirst(usize),
}

pub struct GenerationResult<T> {
    pub draft: T,
    pub timing: Option<GenerationTiming>,
    pub evidence_count: usize,
    /// Whether the selected payload used the preferred layout (rank prefix or optional context).
    pub optional_context: bool,
    /// User message actually selected for completion, when requested by the local client.
    pub completion_prompt: Option<String>,
}

pub enum GenerationAttempt<T> {
    Completed(GenerationResult<T>),
    WithoutDraft(GenerationStop),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GenerationStop {
    ContextLimit,
    Deadline,
    WorkerUnavailable,
    InvalidOutput,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Models {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<Cow<'static, str>>,
    pub profiles: Vec<Model>,
}

pub struct Generation {
    assets: PathBuf,
    runtime: Arc<Runtime>,
    selection_store: Option<Arc<dyn SelectionStore>>,
    state: Mutex<State>,
    operation: Mutex<()>,
}

#[derive(Default)]
struct State {
    models: Vec<(Model, Arc<ManagedWorker>)>,
    selected: Option<usize>,
    request: Option<Arc<Active>>,
}

struct Active {
    id: Option<Uuid>,
    cancelled: AtomicBool,
    model: Model,
    worker: Arc<ManagedWorker>,
}

pub struct Request<'a> {
    started: Instant,
    generation: &'a Generation,
    active: Arc<Active>,
    capture_context: bool,
    _operation: MutexGuard<'a, ()>,
}

impl Generation {
    #[must_use]
    pub fn configured(assets: &Path) -> Option<Self> {
        Self::configured_with_models(assets, Vec::new())
    }

    #[must_use]
    pub fn configured_with_models(assets: &Path, imported: Vec<Model>) -> Option<Self> {
        Self::configured_with_store(assets, imported, None)
    }

    #[must_use]
    pub fn configured_persistent(
        assets: &Path,
        imported: Vec<Model>,
        store: Arc<dyn SelectionStore>,
    ) -> Option<Self> {
        Self::configured_with_store(assets, imported, Some(store))
    }

    fn configured_with_store(
        assets: &Path,
        imported: Vec<Model>,
        selection_store: Option<Arc<dyn SelectionStore>>,
    ) -> Option<Self> {
        let runtime = Arc::new(Runtime::preview(assets));
        if !runtime.is_available() {
            return None;
        }
        Some(Self::with_runtime(
            assets,
            imported,
            selection_store,
            runtime,
        ))
    }

    #[must_use]
    pub fn with_runtime(
        assets: &Path,
        imported: Vec<Model>,
        selection_store: Option<Arc<dyn SelectionStore>>,
        runtime: Arc<Runtime>,
    ) -> Self {
        let models: Vec<_> = MODELS
            .into_iter()
            .chain(imported)
            .filter_map(|model| {
                let worker = ManagedWorker::with_runtime(
                    assets,
                    Profile::Generation(model.clone()),
                    runtime.clone(),
                )?;
                Some((model, Arc::new(worker)))
            })
            .collect();
        let selected = match selection_store
            .as_deref()
            .map_or(Ok(None), SelectionStore::load)
        {
            Ok(saved) => saved
                .and_then(|(id, fingerprint)| {
                    models
                        .iter()
                        .position(|(model, _)| model.id == id && model.fingerprint == fingerprint)
                })
                .or_else(|| (!models.is_empty()).then_some(0)),
            // Keep the available profiles visible for explicit selection when storage cannot be read.
            Err(SelectionStoreError) => None,
        };
        Self {
            assets: assets.to_path_buf(),
            runtime,
            selection_store,
            state: Mutex::new(State {
                models,
                selected,
                request: None,
            }),
            operation: Mutex::default(),
        }
    }

    #[must_use]
    pub fn has_runtime(&self) -> bool {
        self.runtime.is_available()
    }

    /// # Errors
    /// Returns an error if the runtime state cannot be read.
    pub fn models(&self) -> Result<Models, GenerationError> {
        if !self.has_runtime() {
            return Ok(Models::default());
        }
        let state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
        Ok(Self::catalog(&state))
    }

    /// # Errors
    /// Returns an error if the current generation selection cannot be read.
    pub fn active_profile(&self) -> Result<Option<Cow<'static, str>>, GenerationError> {
        let state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
        Ok(state.selected.map(|index| state.models[index].0.id.clone()))
    }

    fn catalog(state: &State) -> Models {
        let profiles: Vec<_> = state
            .models
            .iter()
            .filter(|(_, worker)| worker.weights_exist())
            .map(|(model, _)| model.clone())
            .collect();
        Models {
            active_profile: state
                .selected
                .map(|index| &state.models[index].0.id)
                .filter(|id| profiles.iter().any(|model| &model.id == *id))
                .cloned(),
            profiles,
        }
    }

    /// Runs a local maintenance operation while the requested profile is inactive.
    ///
    /// # Errors
    /// Rejects maintenance on the selected profile or during generation or activation.
    pub fn with_inactive_model<T, E>(
        &self,
        profile: &str,
        maintenance: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, GenerationError> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| GenerationError::Busy)?;
        let mut state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
        if state
            .selected
            .is_some_and(|index| state.models[index].0.id == profile)
        {
            return Err(GenerationError::Busy);
        }
        let result = maintenance();
        if let Some(index) = state.models.iter().position(|(model, worker)| {
            model.id == profile && (result.is_ok() || !worker.weights_exist())
        }) {
            state.models.remove(index).1.shutdown();
            if let Some(selected) = &mut state.selected
                && *selected > index
            {
                *selected -= 1;
            }
        }
        Ok(result)
    }

    /// # Errors
    /// Rejects an unavailable selection or concurrent generation or activation.
    pub fn begin(&self, id: Option<Uuid>) -> Result<Request<'_>, GenerationError> {
        if !self.has_runtime() {
            return Err(GenerationError::Runtime);
        }
        let operation = self
            .operation
            .try_lock()
            .map_err(|_| GenerationError::Busy)?;
        let mut state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
        let selected = state.selected.ok_or(GenerationError::Runtime)?;
        let (model, worker) = &state.models[selected];
        if !worker.weights_exist() {
            return Err(GenerationError::Runtime);
        }
        let active = Arc::new(Active {
            id,
            cancelled: AtomicBool::new(false),
            model: model.clone(),
            worker: worker.clone(),
        });
        state.request = Some(active.clone());
        Ok(Request {
            started: Instant::now(),
            generation: self,
            active,
            capture_context: false,
            _operation: operation,
        })
    }

    /// # Errors
    /// Returns a typed error for an unknown or already completed request.
    pub fn cancel(&self, id: Uuid) -> Result<(), GenerationError> {
        let state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
        let active = state
            .request
            .as_ref()
            .filter(|request| request.id == Some(id))
            .ok_or(GenerationError::UnknownRequest)?;
        active.worker.interrupt(&active.cancelled);
        Ok(())
    }

    /// Warms a provisioned worker before committing the active selection.
    ///
    /// # Errors
    /// Returns an error if another operation is in progress or the target cannot start.
    pub fn select(&self, profile: &str) -> Result<Models, GenerationError> {
        self.select_with_commit(profile, |profile, fingerprint| {
            match &self.selection_store {
                Some(store) => store.save(profile, fingerprint),
                None => Ok(()),
            }
        })
    }

    /// Warm the target and let the caller commit its checked selection atomically.
    ///
    /// # Errors
    /// Returns an error if the profile cannot warm or the commit rejects the selection.
    pub fn select_with_commit(
        &self,
        profile: &str,
        commit: impl FnOnce(&str, &str) -> Result<(), SelectionStoreError>,
    ) -> Result<Models, GenerationError> {
        if !self.has_runtime() {
            return Err(GenerationError::Runtime);
        }
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| GenerationError::Busy)?;
        let (target, previous, worker, fingerprint) = {
            let state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
            let target = state
                .models
                .iter()
                .position(|(model, worker)| model.id == profile && worker.weights_exist())
                .ok_or(GenerationError::UnknownProfile)?;
            (
                target,
                state.selected,
                state.models[target].1.clone(),
                state.models[target].0.fingerprint.clone(),
            )
        };
        worker
            .warm()
            .map_err(|_| GenerationError::ProfileUnavailable)?;
        if commit(profile, &fingerprint).is_err() {
            if previous != Some(target) {
                worker.unload();
            }
            return Err(GenerationError::Runtime);
        }
        let mut state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
        state.selected = Some(target);
        let models = Self::catalog(&state);
        let previous_worker = previous.map(|index| state.models[index].1.clone());
        drop(state);
        if previous != Some(target)
            && let Some(worker) = previous_worker
        {
            worker.unload();
        }
        Ok(models)
    }

    /// Register an installed, verified profile for explicit activation.
    ///
    /// # Errors
    /// Returns an error if the verified model file is unavailable.
    pub fn register(&self, model: Model) -> Result<(), GenerationError> {
        let worker = ManagedWorker::with_runtime(
            &self.assets,
            Profile::Generation(model.clone()),
            self.runtime.clone(),
        )
        .ok_or(GenerationError::Runtime)?;
        let mut state = self.state.lock().map_err(|_| GenerationError::Runtime)?;
        if !state
            .models
            .iter()
            .any(|(existing, _)| existing.id == model.id)
        {
            state.models.push((model, Arc::new(worker)));
        }
        Ok(())
    }

    pub fn shutdown(&self) {
        if let Ok(state) = self.state.lock() {
            for (_, worker) in &state.models {
                worker.shutdown();
            }
        }
    }
}

impl Request<'_> {
    #[must_use]
    pub fn with_context_capture(mut self, enabled: bool) -> Self {
        self.capture_context = enabled;
        self
    }

    #[must_use]
    pub const fn captures_context(&self) -> bool {
        self.capture_context
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.active.model.id
    }

    /// # Errors
    /// Returns cancellation when the active request has been interrupted.
    pub fn check_cancelled(&self) -> Result<(), GenerationError> {
        if self.active.cancelled.load(Ordering::Acquire) {
            Err(GenerationError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Produce an optional search query using the active answer model.
    ///
    /// # Errors
    /// Returns cancellation or admission errors; unavailable or invalid optional output falls back.
    pub fn search_formulation(
        &self,
        question: &str,
        timeout: Duration,
    ) -> Result<Option<String>, GenerationError> {
        let result = self
            .active
            .worker
            .run_interruptible(Some(&self.active.cancelled), |client| {
                let Some(deadline) = Instant::now().checked_add(timeout) else {
                    return Ok(None);
                };
                let client = client.with_deadline(Some(deadline));
                let mut request = payload(self.model(), question, SEARCH_PROMPT, &search_schema());
                request["max_tokens"] = json!(96);
                Ok(
                    generate::<SearchFormulation>(&client, self.model(), &request, 0)
                        .ok()
                        .map(|result| result.draft.query)
                        .filter(|query| !query.trim().is_empty() && query.len() <= 1024),
                )
            });
        if self.active.cancelled.load(Ordering::Acquire) {
            return Err(GenerationError::Cancelled);
        }
        match result {
            Err(GenerationError::Runtime | GenerationError::Output | GenerationError::Deadline) => {
                Ok(None)
            }
            other => other,
        }
    }

    /// # Errors
    /// Returns a typed error when generation was cancelled, the worker failed, or the draft is invalid.
    pub fn generate(
        &self,
        evidence_count: usize,
        prioritize_policy: bool,
        within_request_budget: bool,
        record_context: impl FnOnce(ContextBudget),
        render: impl Fn(usize) -> String,
    ) -> Result<GenerationAttempt<Draft>, GenerationError> {
        self.generate_with_packing(
            PassagePacking::Compact(evidence_count),
            prioritize_policy,
            within_request_budget,
            record_context,
            |count, _| Some(render(count)),
        )
    }

    /// Measure ranked prefixes before trying smaller code passages when allowed.
    ///
    /// # Errors
    /// Returns cancellation, worker admission errors or invalid model output.
    pub fn generate_with_packing(
        &self,
        packing: PassagePacking,
        prioritize_policy: bool,
        within_request_budget: bool,
        record_context: impl FnOnce(ContextBudget),
        render: impl Fn(usize, bool) -> Option<String>,
    ) -> Result<GenerationAttempt<Draft>, GenerationError> {
        let (plan, rank_prefix) = match packing {
            PassagePacking::Compact(count) => ((1..=count).into(), false),
            PassagePacking::RankPrefixFirst(count) => (budget::Plan::RankPrefixFirst(count), true),
        };
        let instruction = answer_prompt(prioritize_policy);
        self.complete(
            plan,
            |count, preferred| {
                if preferred && !rank_prefix {
                    return None;
                }
                render(count, preferred)
            },
            |_| &instruction,
            |count, _| quote_schema(false, count),
            within_request_budget.then(|| self.started + Duration::from_secs(45)),
            record_context,
        )
    }

    /// Try both parts of a declaration and usage within a single local draft.
    ///
    /// # Errors
    /// Returns cancellation or a generation admission error when the request cannot continue.
    pub fn generate_linked(
        &self,
        record_context: impl FnOnce(ContextBudget),
        render: impl Fn() -> String,
    ) -> Result<GenerationAttempt<Draft>, GenerationError> {
        self.check_cancelled()?;
        let now = Instant::now();
        let latest = self.started + Duration::from_secs(35);
        let deadline = latest.min(now + Duration::from_secs(12));
        if now >= deadline {
            return Ok(GenerationAttempt::WithoutDraft(GenerationStop::Deadline));
        }
        let instruction = linked_evidence_prompt();
        let result = self.complete::<LinkedDraft>(
            (2..=2).into(),
            |_, optional| (!optional).then(&render),
            |_| &instruction,
            |count, _| quote_schema(true, count),
            Some(deadline),
            record_context,
        );
        match result {
            Ok(GenerationAttempt::Completed(result)) => {
                Ok(GenerationAttempt::Completed(GenerationResult {
                    draft: Draft {
                        answer: result.draft.answer,
                        citations: result.draft.citations,
                    },
                    timing: result.timing,
                    evidence_count: result.evidence_count,
                    optional_context: result.optional_context,
                    completion_prompt: result.completion_prompt,
                }))
            }
            Ok(GenerationAttempt::WithoutDraft(stop)) => Ok(GenerationAttempt::WithoutDraft(stop)),
            Err(GenerationError::Runtime) => Ok(GenerationAttempt::WithoutDraft(
                GenerationStop::WorkerUnavailable,
            )),
            Err(GenerationError::Output) => Ok(GenerationAttempt::WithoutDraft(
                GenerationStop::InvalidOutput,
            )),
            Err(GenerationError::Deadline) => {
                Ok(GenerationAttempt::WithoutDraft(GenerationStop::Deadline))
            }
            Err(error) => Err(error),
        }
    }

    /// # Errors
    /// Returns a typed error when generation was cancelled, failed, or returned an invalid selection.
    pub fn compare(
        &self,
        pairs: usize,
        record_context: impl FnOnce(ContextBudget),
        render: impl Fn(usize, bool) -> Option<String>,
    ) -> Result<GenerationAttempt<ComparisonDraft>, GenerationError> {
        self.complete(
            (1..=pairs).into(),
            render,
            comparison_prompt,
            comparison_schema,
            None,
            record_context,
        )
    }

    /// Try a second constrained draft within the original generation time budget.
    /// # Errors
    /// Returns cancellation or a worker error while time remains.
    pub fn compare_remaining(
        &self,
        pairs: usize,
        record_context: impl FnOnce(ContextBudget),
        render: impl Fn(usize, bool) -> Option<String>,
    ) -> Result<GenerationAttempt<ComparisonDraft>, GenerationError> {
        self.check_cancelled()?;
        let deadline = self.started + Duration::from_secs(45);
        if Instant::now() >= deadline {
            return Ok(GenerationAttempt::WithoutDraft(GenerationStop::Deadline));
        }
        let result = self.complete(
            (1..=pairs).into(),
            render,
            comparison_prompt,
            comparison_schema,
            Some(deadline),
            record_context,
        );
        match result {
            Err(GenerationError::Deadline) => {
                Ok(GenerationAttempt::WithoutDraft(GenerationStop::Deadline))
            }
            Err(GenerationError::Runtime) if Instant::now() >= deadline => Ok(
                GenerationAttempt::WithoutDraft(GenerationStop::WorkerUnavailable),
            ),
            result => result,
        }
    }

    fn complete<'a, T: serde::de::DeserializeOwned>(
        &self,
        plan: budget::Plan,
        render: impl Fn(usize, bool) -> Option<String>,
        system: impl Fn(bool) -> &'a str,
        schema: impl Fn(usize, bool) -> Value,
        deadline: Option<Instant>,
        record_context: impl FnOnce(ContextBudget),
    ) -> Result<GenerationAttempt<T>, GenerationError> {
        let result =
            self.active
                .worker
                .run_until(Some(&self.active.cancelled), deadline, |client| {
                    let model = self.model();
                    let (budget, payload) = budget::fit(
                        client,
                        model,
                        &self.active.cancelled,
                        plan,
                        |count, optional| {
                            render(count, optional).map(|prompt| {
                                payload(model, &prompt, system(optional), &schema(count, optional))
                            })
                        },
                    )?;
                    // Budget probes and the selected completion payload are distinct.
                    record_context(budget);
                    let Some((mut payload, optional)) = payload else {
                        return Ok(GenerationAttempt::WithoutDraft(
                            GenerationStop::ContextLimit,
                        ));
                    };
                    let mut result = generate(client, model, &payload, budget.admitted)?;
                    result.optional_context = optional;
                    if self.capture_context
                        && let Value::String(prompt) = payload["messages"][1]["content"].take()
                    {
                        result.completion_prompt = Some(prompt);
                    }
                    Ok(GenerationAttempt::Completed(result))
                });
        if self.active.cancelled.load(Ordering::Acquire) {
            return Err(GenerationError::Cancelled);
        }
        result
    }

    /// Linearizes normal completion against cancellation.
    ///
    /// # Errors
    /// Returns a cancellation error if cancellation won the race.
    pub fn finish(self) -> Result<(), GenerationError> {
        let mut state = self
            .generation
            .state
            .lock()
            .map_err(|_| GenerationError::Runtime)?;
        state.request = None;
        if self.active.cancelled.load(Ordering::Acquire) {
            return Err(GenerationError::Cancelled);
        }
        Ok(())
    }
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.generation.state.lock() {
            state.request = None;
        }
    }
}

fn payload(model: &str, prompt: &str, system: &str, schema: &Value) -> Value {
    json!({
        "model": model,
        "temperature": 0,
        "max_tokens": budget::OUTPUT_TOKENS,
        "stream": false,
        "messages": [
            {"role":"system","content":system},
            {"role":"user","content":prompt}
        ],
        "response_format": {"type":"json_schema","schema":schema}
    })
}

fn generate<T: serde::de::DeserializeOwned>(
    client: &Client,
    model: &str,
    payload: &Value,
    evidence_count: usize,
) -> Result<GenerationResult<T>, GenerationError> {
    let bytes = client.post("/v1/chat/completions", payload)?;
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| GenerationError::Output)?;
    let choice = &response["choices"][0];
    if response["model"] != model || choice["finish_reason"] != "stop" {
        return Err(GenerationError::Output);
    }
    let text = choice["message"]["content"]
        .as_str()
        .ok_or(GenerationError::Output)?;
    Ok(GenerationResult {
        draft: serde_json::from_str(text).map_err(|_| GenerationError::Output)?,
        timing: GenerationTiming::from_llama(&response["timings"]),
        evidence_count,
        optional_context: false,
        completion_prompt: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnreadableSelection;

    impl SelectionStore for UnreadableSelection {
        fn load(&self) -> Result<Option<(String, String)>, SelectionStoreError> {
            Err(SelectionStoreError)
        }

        fn save(&self, _: &str, _: &str) -> Result<(), SelectionStoreError> {
            Err(SelectionStoreError)
        }
    }

    #[test]
    fn unreadable_preference_does_not_select_a_different_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        for file in ["llama-server", ALTERNATE_FILE] {
            std::fs::write(directory.path().join(file), b"fixture")?;
        }
        let generation = Generation::configured_persistent(
            directory.path(),
            Vec::new(),
            Arc::new(UnreadableSelection),
        )
        .ok_or("missing generation")?;

        let models = generation.models()?;
        assert_eq!(models.profiles.len(), 1);
        assert_eq!(models.profiles[0].id, ALTERNATE_PROFILE);
        assert_eq!(models.active_profile, None);
        assert!(matches!(
            generation.begin(None),
            Err(GenerationError::Runtime)
        ));
        Ok(())
    }

    #[test]
    fn missing_selected_weights_prevent_a_new_request() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        for file in ["llama-server", GENERATION_FILE, ALTERNATE_FILE] {
            std::fs::write(directory.path().join(file), b"fixture")?;
        }
        let generation = Generation::configured(directory.path()).ok_or("missing generation")?;
        assert_eq!(generation.begin(None)?.model(), GENERATION_PROFILE);
        std::fs::remove_file(directory.path().join(GENERATION_FILE))?;
        let models = generation.models()?;
        assert_eq!(models.profiles.len(), 1);
        assert_eq!(models.profiles[0].id, ALTERNATE_PROFILE);
        assert_eq!(models.active_profile, None);
        assert!(matches!(
            generation.begin(None),
            Err(GenerationError::Runtime)
        ));
        Ok(())
    }

    #[test]
    fn optional_attempts_distinguish_expired_request_budget_from_context_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        for file in ["llama-server", GENERATION_FILE] {
            std::fs::write(directory.path().join(file), b"fixture")?;
        }
        let generation = Generation::configured(directory.path()).ok_or("missing generation")?;
        let id = Uuid::from_u128(1);
        let mut request = generation.begin(Some(id))?;
        request.started = Instant::now()
            .checked_sub(Duration::from_secs(46))
            .ok_or("clock")?;
        assert!(matches!(
            request.generate_linked(|_| unreachable!(), || unreachable!()),
            Ok(GenerationAttempt::WithoutDraft(GenerationStop::Deadline))
        ));
        assert!(matches!(
            request.compare_remaining(1, |_| unreachable!(), |_, _| unreachable!()),
            Ok(GenerationAttempt::WithoutDraft(GenerationStop::Deadline))
        ));
        assert!(matches!(
            request.generate(1, false, true, |_| unreachable!(), |_| unreachable!()),
            Err(GenerationError::Deadline)
        ));
        generation.cancel(id)?;
        assert!(matches!(
            request.generate_linked(|_| unreachable!(), || unreachable!()),
            Err(GenerationError::Cancelled)
        ));
        assert!(matches!(
            request.compare_remaining(1, |_| unreachable!(), |_, _| unreachable!()),
            Err(GenerationError::Cancelled)
        ));
        assert_eq!(request.finish(), Err(GenerationError::Cancelled));
        Ok(())
    }

    #[test]
    fn failed_removal_keeps_intact_model_and_forgets_a_missing_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        for file in ["llama-server", GENERATION_FILE, ALTERNATE_FILE] {
            std::fs::write(directory.path().join(file), b"fixture")?;
        }
        let generation = Generation::configured(directory.path()).ok_or("missing generation")?;
        let failed =
            generation.with_inactive_model(ALTERNATE_PROFILE, || Err::<(), _>("denied"))?;
        assert_eq!(failed, Err("denied"));
        assert!(
            generation
                .models()?
                .profiles
                .iter()
                .any(|model| model.id == ALTERNATE_PROFILE)
        );
        let failed = generation.with_inactive_model(ALTERNATE_PROFILE, || {
            std::fs::remove_file(directory.path().join(ALTERNATE_FILE))?;
            Err::<(), _>(std::io::Error::other("registry unavailable"))
        })?;
        assert!(failed.is_err());
        std::fs::write(directory.path().join(ALTERNATE_FILE), b"unverified")?;
        assert!(
            !generation
                .models()?
                .profiles
                .iter()
                .any(|model| model.id == ALTERNATE_PROFILE)
        );
        Ok(())
    }
}
