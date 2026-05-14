## Goal

Set up the initial Rust gRPC scaffolding for the ledger service. This covers protobuf codegen via `tonic-build`, a `tokio` runtime entry point, and `LedgerService` handlers that return deterministic stub data. No persistence, no actor wiring, no validation logic at this stage. The output of this plan is a runnable binary that accepts gRPC traffic on a configured port.

Reference: [[Phase 1 - Rust]], [[Phase 1]] (sections "Protobuf" and "Handlers").

## Scope

In scope:
1. Cargo dependency setup (`tokio`, `thiserror`, plus the existing `tonic` / `prost` / `tonic-build`).
2. `build.rs` that compiles `proto/ledger.proto` into Rust types via `tonic-build`.
3. Module layout under `rust/src/` to host generated code, handlers, and an error type.
4. `LedgerService` implementation with stubbed responses for both RPCs.
5. `main.rs` that boots the `tokio` runtime and serves the gRPC service on a fixed local port.

Out of scope:
1. Book registry, acceptor, writer, applier, provisioner actors. These remain pseudocode in [[Phase 1]].
2. Request validation rules listed in [[Phase 1]] § Financial Operation Request.
3. Persistence files (`.cov`, `.seg`).
4. Configuration loading. The port stays hard-coded for this phase.
5. Logging or tracing setup beyond `println!`-level startup output.

## Target Directory Structure

```
rust/
├── Cargo.toml
├── build.rs
└── src/
    ├── main.rs
    ├── proto.rs          // tonic::include_proto! wrapper
    ├── service.rs        // LedgerService implementation with stubs
    └── error.rs          // ServiceError via thiserror
```

The `proto/ledger.proto` file stays where it is (`proto/ledger.proto` at the repo root). `build.rs` references it through a relative path.

## Step-by-step

### Step 1: Update `rust/Cargo.toml`

Add the runtime and error crates, and request the codegen features that `tonic` needs.

```toml
[package]
name = "ledger-api"
version = "0.1.0"
edition = "2024"

[dependencies]
prost = "0.14.3"
tonic = "0.14.6"
tonic-prost = "0.14.6"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal"] }
thiserror = "1"

[build-dependencies]
tonic-prost-build = "0.14.6"
```

Notes:
1. `signal` feature on `tokio` is included now so we can configure graceful shutdown later without a churned `Cargo.toml`. If we want to defer, drop `signal` and add it when needed.
2. `prost` stays explicit because handler code may need to reference its derive output directly.
3. In tonic 0.14 the prost integration was extracted out of `tonic-build` into a new `tonic-prost-build` crate. The build script must therefore depend on `tonic-prost-build` (build-only) and the runtime must depend on `tonic-prost` for the prost codec. The plain `tonic-build` crate no longer exposes `configure()` and produces an E0425 if used.

### Step 2: Add `rust/build.rs`

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_file = "../proto/ledger.proto";
    let proto_dir = "../proto";

    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&[proto_file], &[proto_dir])?;

    println!("cargo:rerun-if-changed={proto_file}");
    Ok(())
}
```

Notes:
1. `build_client(false)` keeps the generated module surface small. We have no in-process client today.
2. The `cargo:rerun-if-changed` directive forces regeneration when the proto file changes.
3. Generated code lands in `OUT_DIR`. We expose it through `tonic::include_proto!("ledger")` in the next step.

### Step 3: Add `rust/src/proto.rs`

Single-purpose module that exposes the generated types under one path.

```rust
tonic::include_proto!("ledger");
```

This puts every message and the `ledger_service_server` module under `crate::proto`. Handler code will reach into `crate::proto::ledger_service_server::{LedgerService, LedgerServiceServer}` and the request and response message types.

### Step 4: Add `rust/src/error.rs`

A minimal error type that handler code can use today and that future actor wiring can extend.

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("internal failure: {0}")]
    Internal(String),
}

impl From<ServiceError> for tonic::Status {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::InvalidRequest(msg) => tonic::Status::invalid_argument(msg),
            ServiceError::Internal(msg) => tonic::Status::internal(msg),
        }
    }
}
```

Stub handlers in this phase will not produce errors. The type exists so later phases can plug in `?` propagation without restructuring.

### Step 5: Add `rust/src/service.rs`

Implement the generated `LedgerService` trait with stub responses. Both handlers ignore request contents in this phase. Stub values are chosen to be obviously synthetic so any test using them is easy to spot.

```rust
use tonic::{Request, Response, Status};

use crate::proto::{
    ledger_service_server::LedgerService,
    BookBalance, CreateAccountRequest, CreateAccountResponse,
    FinancialOperationRequest, FinancialOperationResponse,
};

#[derive(Debug, Default)]
pub struct LedgerApi;

#[tonic::async_trait]
impl LedgerService for LedgerApi {
    async fn create_account(
        &self,
        _request: Request<CreateAccountRequest>,
    ) -> Result<Response<CreateAccountResponse>, Status> {
        Ok(Response::new(CreateAccountResponse { book_id: 1 }))
    }

    async fn execute_operation(
        &self,
        request: Request<FinancialOperationRequest>,
    ) -> Result<Response<FinancialOperationResponse>, Status> {
        let entries = request
            .into_inner()
            .command
            .map(|cmd| cmd.entries)
            .unwrap_or_default();

        let balances = entries
            .into_iter()
            .map(|entry| BookBalance {
                book_id: entry.book_id,
                ending_balance: 0,
            })
            .collect();

        Ok(Response::new(FinancialOperationResponse {
            operation_id: 1,
            timestamp_ns: 0,
            balances,
        }))
    }
}
```

Notes:
1. `execute_operation` echoes the input `book_id`s with a zero balance. This makes the stub useful for round-trip testing without implying any real semantics.
2. We avoid `unwrap` per the project convention. The `command` field is optional in the generated type, so we use `map(...).unwrap_or_default()` to handle a missing payload safely.

### Step 6: Add `rust/src/main.rs`

```rust
mod error;
mod proto;
mod service;

use tonic::transport::Server;

use crate::proto::ledger_service_server::LedgerServiceServer;
use crate::service::LedgerApi;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:50051".parse()?;
    let api = LedgerApi::default();

    println!("ledger-api listening on {addr}");

    Server::builder()
        .add_service(LedgerServiceServer::new(api))
        .serve(addr)
        .await?;

    Ok(())
}
```

Notes:
1. Address is hard-coded for this phase. A later plan will extract it into config.
2. `Box<dyn std::error::Error>` at `main` is acceptable here because there is no upstream caller. Service-internal code routes through `ServiceError` instead.

### Step 7: Verify the build and a manual round-trip

1. From the `rust/` directory, run `cargo build` to confirm codegen and compilation succeed.
2. Run `cargo run` to start the server. Expect `ledger-api listening on 127.0.0.1:50051`.
3. Use `grpcurl` for a manual smoke test:
   ```
   grpcurl -plaintext -import-path ../proto -proto ledger.proto \
     -d '{"command": {"accounting_type": "DEBIT", "balance_type": "AVAILABLE", "allow_overdraft": false}}' \
     127.0.0.1:50051 ledger.LedgerService/CreateAccount

   grpcurl -plaintext -import-path ../proto -proto ledger.proto \
     -d '{"command": {"entries": [{"book_id": 1, "accounting_type": "DEBIT", "amount": 100, "ledger_code": "TEST"}]}}' \
     127.0.0.1:50051 ledger.LedgerService/ExecuteOperation
   ```
4. Expected responses:
   1. `{"book_id": 1}`
   2. `{"operation_id": "1", "timestamp_ns": "0", "balances": [{"book_id": 1, "ending_balance": "0"}]}`

`grpcurl` is optional and is only used to confirm the server responds. CI integration will come in a later phase.

## Risks and open questions

1. **Edition 2024 + `tonic` macro hygiene.** The existing `Cargo.toml` pins `edition = "2024"`. `#[tonic::async_trait]` and the generated code should work, though if we hit a macro issue we fall back to `edition = "2021"`. Decision on fallback can wait until we observe a real build failure.
2. **Proto path stability.** `build.rs` depends on the relative path `../proto`. If the workspace layout changes (for example, a Cargo workspace at the repo root), we revisit the path then.
3. **Stub semantics.** `execute_operation` returning a single fixed `operation_id: 1` is fine for stubs but will collide with the real journal counter once persistence lands. Replace at that step.

## Definition of Done

1. `cargo build` succeeds in `rust/`.
2. `cargo run` starts a tonic server bound to `127.0.0.1:50051`.
3. Both `LedgerService` RPCs return the stub responses described in Step 5.
4. `Phase 1 - Rust.md` has a wikilink to this plan.
5. No persistence, validation, or actor logic is introduced.
