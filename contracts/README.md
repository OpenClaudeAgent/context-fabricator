# Contracts

[`openapi.yaml`](openapi.yaml) is the canonical versioned loopback API. The Swift client target references this file directly. Axum implements the server boundary. `make integration` validates the schema and the running backend and exercises a breaking path change.
