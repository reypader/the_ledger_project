## Goal

Introduce the five system actors from [[Phase 1]] (Book Manager, Acceptor, Writer, Applier, Provisioner) as stub `tokio` tasks. Each actor owns an `mpsc` channel and processes its message enum, but the message handlers return fixed affirmative results or simply drop the message. The gRPC handlers in [[Phase 1 - Rust - gRPC Scaffolding]] are rewired so they reach the actors through channels instead of returning hard-coded responses inline. No persistence, no validation rules, no real registry.

Reference: [[Phase 1 - Rust]], [[Phase 1]] (sections "Handlers", "Book Manager", "Acceptor", "Writer", "Applier", "Provisioner").

## Scope

In scope:
1. New `actors` module hierarchy: one file per actor, plus a shared `mod.rs`.
2. Message enums and `Handle` structs for each actor, with a `spawn()` constructor that returns the handle.
3. Stub behavior for each actor as described in the [[#Stub Behavior]] section.
4. Startup wiring in `main.rs`: spawn every actor, collect handles into an `AppState`, and share that with the gRPC service.
5. Rewrite `service.rs` handlers so they send messages and await responses through the handles.

Out of scope:
1. Request validation rules listed in [[Phase 1]] § Financial Operation Request. Handlers accept any well-formed protobuf payload.
2. The book registry cache (`HashMap`/`Weak<BookState>`) and its periodic eviction. The handler always asks Book Manager for a `BookState` per request.
3. Journal traversal at startup and the operation/book id counters that come out of it. The Book Manager seeds its in-memory `book_id` counter from `1`, and the Acceptor seeds its `operation_id` counter from `1`.
4. Real persistence. Writer, Applier, and Provisioner do no disk IO at this phase.
5. Graceful shutdown, system-failure paths, and the `flush_timer` / `apply_dispatch_interval` periodic tasks.
6. Logging or tracing setup. `println!` at actor startup is acceptable.

## Target Directory Structure

```
rust/
├── Cargo.toml
├── build.rs
└── src/
    ├── main.rs
    ├── proto.rs
    ├── service.rs
    ├── error.rs
    ├── actors.rs            // re-exports submodules and AppState
    └── actors/
        ├── book_manager.rs
        ├── acceptor.rs
        ├── writer.rs
        ├── applier.rs
        └── provisioner.rs
```

## Shared Types

A handful of types are referenced by more than one actor. They live in `actors.rs` so each actor module can `use crate::actors::{...}` without circular imports.

```rust
use std::sync::Arc;
use tokio::sync::oneshot;

use crate::proto::{AccountType, BalanceType};

#[derive(Debug, Clone, Copy)]
pub struct BookState {
    pub id: u32,
    pub accounting_type: AccountType,
    pub balance_type: BalanceType,
    pub allow_overdraft: bool,
    pub projected_balance: i128,
    pub latest_segment: u32,
    pub latest_offset: u32,
}

#[derive(Debug)]
pub struct PreprocessedBook {
    pub book_state: Arc<BookState>,
    pub incoming_total: i64,
    pub ops: Vec<PreprocessedEntry>,
}

#[derive(Debug)]
pub struct PreprocessedEntry {
    pub signed_amount: i64,
    pub ledger_code: [u8; 8],
}

#[derive(Debug)]
pub struct AcceptedOperation {
    pub operation_id: u64,
    pub timestamp_ns: u128,
    pub ending_balances: Vec<(u32, i64)>,
}

#[derive(Debug)]
pub enum AcceptResult {
    Accepted(AcceptedOperation),
    Rejected { book_id: u32 },
    Failure(String),
}

pub type AcceptResponder = oneshot::Sender<AcceptResult>;
```

Notes:
1. `BookState` is `Copy` here because it has no owned heap data in this phase. Once the real Book Manager populates it from a journal, this may change; the handler keeps it behind `Arc` so we can swap the internals without touching call sites.
2. `AcceptResult::Failure` covers the "channel closed" cases listed in [[Phase 1]] § System Failure even though the stubs never produce one.

The `AppState` aggregates every handle so `service.rs` only needs one field.

```rust
#[derive(Clone)]
pub struct AppState {
    pub book_manager: book_manager::Handle,
    pub acceptor: acceptor::Handle,
    pub writer: writer::Handle,
    pub applier: applier::Handle,
    pub provisioner: provisioner::Handle,
}
```

## Actor Module Template

Every actor module follows the same shape. Example skeleton:

```rust
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum Message {
    // variants per actor
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Message>,
}

impl Handle {
    pub async fn send(&self, msg: Message) -> Result<(), mpsc::error::SendError<Message>> {
        self.tx.send(msg).await
    }
}

pub fn spawn() -> Handle {
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            handle(msg).await;
        }
    });
    Handle { tx }
}

async fn handle(msg: Message) {
    // stub body per actor
}
```

Notes:
1. Channel capacity is fixed at `256` for every actor. Tuning is deferred.
2. `Handle` is `Clone` so the gRPC service and other actors can share it freely.
3. Each actor exposes typed convenience methods on `Handle` (for example `book_manager::Handle::create(...)`) that build the `Message` variant and await the `oneshot::Receiver`. These wrappers keep call sites in `service.rs` short.

## Stub Behavior

### Book Manager — `actors/book_manager.rs`

Messages:
1. `Create { accounting_type, balance_type, allow_overdraft, respond_to: oneshot::Sender<u32> }`
2. `LoadBook { book_id, respond_to: oneshot::Sender<Arc<BookState>> }`

Stub behavior:
1. The task owns a `u32` counter initialized to `1`. `Create` increments it after sending the current value back. The actor also stashes the requested `accounting_type`, `balance_type`, and `allow_overdraft` in a small in-memory `HashMap<u32, (AccountType, BalanceType, bool)>` so `LoadBook` can echo them.
2. `LoadBook` constructs a `BookState` for the requested id with `projected_balance = 0`, `latest_segment = 1`, `latest_offset = 0`. If the id was never created in this process, the stub falls back to `(DEBIT, AVAILABLE, false)`.
3. No `Periodic Registry Eviction` task. The HashMap is owned by the task and never shrinks.

### Acceptor — `actors/acceptor.rs`

Messages:
1. `Accept { books: Vec<PreprocessedBook>, respond_to: AcceptResponder }`

Stub behavior:
1. The task owns a `u64` operation id counter starting at `1`.
2. On `Accept`, the stub does not validate `allow_overdraft` or run a real commit pass. It produces an `AcceptedOperation` whose `ending_balances` reuses each book's `projected_balance + incoming_total` truncated to `i64`. `operation_id` comes from the counter, `timestamp_ns` from `std::time::SystemTime::now()`.
3. The stub forwards an internal `writer::Message::Ack` carrying the `AcceptedOperation` and the original `respond_to` to the Writer handle. The Writer is the one that fulfills the oneshot. This keeps the channel topology faithful to the design even though no batching happens.
4. If the writer channel is closed, the stub itself sends `AcceptResult::Failure(...)` on `respond_to`.

### Writer — `actors/writer.rs`

Messages:
1. `Ack { accepted: AcceptedOperation, respond_to: AcceptResponder }`

Stub behavior:
1. On every `Ack`, immediately send `AcceptResult::Accepted(accepted)` through `respond_to`. No batching, no flush timer, no IO.
2. After answering the responder, fire a single `applier::Message::Apply` carrying the `operation_id` and the affected `book_id` list. The Applier drops it. This exists so the channel link is exercised once per request.
3. No `Periodically dispatch Apply()` task. The plan does not introduce any timers.

### Applier — `actors/applier.rs`

Messages:
1. `Apply { operation_id: u64, book_ids: Vec<u32> }`
2. `Ping { respond_to: oneshot::Sender<()> }`

Stub behavior:
1. `Apply` is logged at most once per actor lifetime via a counter, then dropped. We avoid logging on every message to keep stub output quiet.
2. `Ping` immediately returns `()`. The future Step 4 of [[Phase 1]] § On Startup will use this; we expose it now so the contract is real even though no current caller waits for it.

### Provisioner — `actors/provisioner.rs`

Messages:
1. `PreProvisionOperationSegment { segment_number: u32 }`
2. `PreProvisionBookSegment { book_id: u32, segment_number: u32 }`

Stub behavior:
1. Both messages are dropped. No oneshot is involved because the design treats both as fire-and-forget at this stage. The Applier waits for completion via a separate path that is not introduced in this plan.

## Handler Rewrite — `service.rs`

The `LedgerApi` struct gains an `AppState` field.

```rust
pub struct LedgerApi {
    state: AppState,
}

impl LedgerApi {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}
```

### `create_account`

1. Pull `accounting_type`, `balance_type`, `allow_overdraft` from the request `command`. Missing command falls back to `(UNSPECIFIED, UNSPECIFIED, false)`.
2. Build a `oneshot` and send `Create { ..., respond_to }` to `state.book_manager`.
3. Await the oneshot. Map a closed sender to `ServiceError::Internal("book manager unavailable")`.
4. Return `CreateAccountResponse { book_id }`.

### `execute_operation`

1. Pull the entries vector from the request. An empty vector still flows through.
2. For every distinct `book_id` in the entries, send a `LoadBook` to the Book Manager and await the `Arc<BookState>`. The loads run in parallel via `futures::future::try_join_all` over the oneshot receivers.
3. Build `PreprocessedBook` values: group entries by `book_id`, compute `signed_amount` by comparing the entry's `accounting_type` to the book's, accumulate `incoming_total`, and copy the first 8 bytes of `ledger_code` into a `[u8; 8]` (right-padded with zero).
4. Send `Accept { books, respond_to }` to the Acceptor and await the result.
5. Map `AcceptResult::Accepted` into `FinancialOperationResponse`. `AcceptResult::Rejected` becomes `ServiceError::InvalidRequest`. `AcceptResult::Failure` becomes `ServiceError::Internal`.

Notes:
1. `futures` is added to `Cargo.toml` (`futures = "0.3"`) only if we use `try_join_all`. If we'd rather not pull in `futures`, we can iterate with `tokio::join!` after collecting receivers, or loop sequentially since payloads are small. Recommendation is to use sequential await for now to keep the dependency set unchanged.
2. The handler still does no validation. Negative amounts, oversized ledger codes, and unknown books are accepted because the Book Manager stub fabricates a `BookState` for any id.

## Startup Wiring — `main.rs`

```rust
mod actors;
mod error;
mod proto;
mod service;

use tonic::transport::Server;

use crate::actors::{AppState, acceptor, applier, book_manager, provisioner, writer};
use crate::proto::ledger_service_server::LedgerServiceServer;
use crate::service::LedgerApi;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provisioner = provisioner::spawn();
    let applier = applier::spawn();
    let writer = writer::spawn(applier.clone());
    let acceptor = acceptor::spawn(writer.clone());
    let book_manager = book_manager::spawn();

    let state = AppState {
        book_manager,
        acceptor,
        writer,
        applier,
        provisioner,
    };

    let addr = "127.0.0.1:50051".parse()?;
    println!("ledger-api listening on {addr}");

    Server::builder()
        .add_service(LedgerServiceServer::new(LedgerApi::new(state)))
        .serve(addr)
        .await?;

    Ok(())
}
```

Notes:
1. The `spawn` order matters because `acceptor::spawn` and `writer::spawn` take the downstream handle as an argument. Spawning bottom-up avoids needing a separate `wire(...)` step.
2. No graceful shutdown. Pressing Ctrl+C terminates the process and orphans the channels; this is acceptable for a stub phase.

## Cargo.toml Changes

No new dependencies are required if the handler uses sequential awaits. The existing `tokio` feature set (`macros`, `rt-multi-thread`, `signal`) covers `mpsc`, `oneshot`, and `spawn`.

If we adopt `try_join_all`, add:

```toml
futures = "0.3"
```

The plan recommends staying with sequential awaits in this phase and revisiting parallelism once real persistence makes loads expensive.

## Step-by-step

1. Create `rust/src/actors.rs` with the shared types (`BookState`, `PreprocessedBook`, `PreprocessedEntry`, `AcceptedOperation`, `AcceptResult`, `AcceptResponder`) and the `AppState` struct. Re-export each submodule with `pub mod ...`.
2. Add `rust/src/actors/book_manager.rs` with `Message`, `Handle`, `spawn()`, and the in-memory map described in [[#Book Manager — actors/book_manager.rs]].
3. Add `rust/src/actors/provisioner.rs` and `rust/src/actors/applier.rs`. These are the simplest because they have no return values for most variants.
4. Add `rust/src/actors/writer.rs`. Its `spawn(applier: applier::Handle) -> Handle` signature captures the downstream link.
5. Add `rust/src/actors/acceptor.rs`. Its `spawn(writer: writer::Handle) -> Handle` signature captures the downstream link.
6. Update `rust/src/service.rs` to take `AppState`, remove the inline stub responses, and implement the message flow described in [[#Handler Rewrite — service.rs]].
7. Update `rust/src/main.rs` to spawn the actors and build `AppState` before constructing `LedgerApi`.
8. Run `cargo build` in `rust/` to confirm everything compiles.
9. Run `cargo run` and repeat the `grpcurl` smoke tests from [[Phase 1 - Rust - gRPC Scaffolding]]. Expected responses:
   1. `CreateAccount` returns `{"book_id": <n>}` with `<n>` incrementing per call.
   2. `ExecuteOperation` returns `{"operation_id": "<n>", "timestamp_ns": "<now>", "balances": [...]}` where each `ending_balance` is `incoming_total` for that book in the request.

## Risks and open questions

1. **Stub `ending_balance` semantics.** The stub returns `incoming_total` per book, which is a useful echo for tests but is not the value the real Acceptor will produce once `projected_balance` is non-zero. Anything relying on zero balances from the previous gRPC stub will need to update. Acceptable churn since the previous scaffolding plan was explicit that stubs are not stable.
2. **`Arc<BookState>` vs `BookState` value.** We pass `Arc<BookState>` so the eventual registry can keep a single owner. If profiling shows `Arc` overhead during high-fanout requests, we revisit the type once real state is in place.
3. **Sequential `LoadBook` loop.** Acceptable while books fabricate instantly. When loads hit disk, we either move to `try_join_all` or batch loads in a single `LoadBooks` message.
4. **No backpressure handling.** Every `send` is awaited but a `SendError` (channel closed) is mapped to `ServiceError::Internal`. The richer system-failure semantics from [[Phase 1]] § System Failure are deferred.

## Definition of Done

1. `cargo build` succeeds in `rust/`.
2. `cargo run` starts the server and prints actor startup lines plus the existing `ledger-api listening on 127.0.0.1:50051` line.
3. `CreateAccount` returns an incrementing `book_id` across calls within one process lifetime.
4. `ExecuteOperation` returns a synthetic `operation_id`, current `timestamp_ns`, and one `BookBalance` per distinct `book_id` in the request.
5. Killing the gRPC client mid-call does not crash the server. (Manual check; no test harness yet.)
6. `Phase 1 - Rust.md` has a wikilink to this plan.
7. No persistence, validation, or real actor logic is introduced beyond what is listed in [[#Stub Behavior]].
