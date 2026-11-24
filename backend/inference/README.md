# Inference

`ctxfab-inference` is a member of the backend Cargo workspace. Its [embedding contract](src/lib.rs) accepts a passage or query and returns a normalized vector or a typed error. The [Qwen3 profile](src/qwen3.rs) owns the model identifier, input formatting and byte budget. The [`llama.cpp` adapter](src/llama_cpp.rs) formats embedding requests and validates the returned profile and vectors.

The [server composition root](../server/src/main.rs) selects the adapter. The [retrieval use case](../server/src/application/retrieval.rs) depends on the embedding contract and owns rebuild, ranking and provenance through its repository and vector-index ports.

The macOS app contains a verified llama.cpp runtime. The Qt Ubuntu package pairs the backend with the pinned CPU runtime. Model weights live in the private profile. The app and CLI can install verified profiles while the backend is running. A development session can use `CTXFAB_INFERENCE_DIRECTORY` to point at a staged fixture and weights. [Runtime packaging](../../ARCHITECTURE.md#local-inference)

The [supervisor](src/supervisor.rs) exposes three internal responsibilities.

- [Worker lifecycle](src/supervisor/worker.rs) admits one operation per profile, starts lazily, and owns cancellation, unloading, and recovery. A failed operation triggers a bounded readiness probe. If the worker is unreachable, the supervisor retires it before releasing admission so the next request can start a fresh worker.
- [HTTP client](src/supervisor/http.rs) owns authenticated loopback requests, deadlines, body limits, and readiness verification against the expected profile alias.
- [Guardian](src/supervisor/guardian.rs) launches the native worker and reaps it when the backend's parent pipe closes. The pipe also carries the worker credential, which the guardian stores in a private file for the native process.

[Generation](src/generation.rs) sends a bounded schema-constrained request and returns a draft. The backend [answer use case](../server/src/application/answers.rs) owns evidence selection, exact-quote verification, and provenance checks.

The [generation controller](src/generation.rs) admits a single answer or profile activation at a time. A fresh request UUID lets a client cancel that answer before it commits. The controller warms a provisioned generation profile before switching the session to it and releases the previous worker after success. A failed activation preserves the previous selection. The embedding worker has a separate lifetime and index.

The [answer qualification harness](../../quality/answers/check.sh) exercises app-owned Q4_K_M and Q5_K_M weights, cancellation and model switching. The [benchmark guide](../../quality/github/README.md) describes the retrieval and answer fixtures. [Release preparation](../../docs/RELEASE.md)
