# Contracts

[`openapi.yaml`](openapi.yaml) is the canonical versioned loopback API. Swift and Qt generate their local API clients from this schema. Axum implements the Rust server boundary. Run `make check-contract` to validate the schema and its compatibility with the running backend. [Swift client](../desktop/macos/Client/) · [Qt generation](../desktop/qt/README.md#build)

[`baselines/v1.yaml`](baselines/v1.yaml) is the frozen v1 reference used to detect breaking changes.
