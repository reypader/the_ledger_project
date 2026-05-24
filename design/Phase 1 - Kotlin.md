## Implementation Guidelines
- Drive graceful shutdown through a dedicated signal, not through `Job`/scope cancellation. Coroutine cancellation throws `CancellationException` at the next suspension point, which can land inside message processing (a suspend send to a downstream actor, suspend IO, an `await`) and interrupt it. That breaks the "do not interrupt processing" assumption. Reserve cancellation for the hard-abort System Failure path only.
- Run each actor loop with `select {}` over a dedicated shutdown source plus the work channel, for example `select { shutdownChannel.onReceive {...}; workChannel.onReceiveCatching { processFully(it) } }`. `select` is biased to the first clause when several are ready, so list shutdown first. This mirrors `biased;` in Rust's `select!`.
    - for the `Acceptor`, `Accept` takes precedence over the `flush_timer`
    - for the `Applier`, provisioning book segments takes precedence over operation segments so operations are backpressured.
- If cancellation is ever in play around an actor, wrap per-message work in `withContext(NonCancellable) { ... }` so a message that has started always runs to completion.
- For the gRPC layer (first in the shutdown order), use `Server.shutdown()` (grpc-kotlin/grpc-java) to stop accepting new RPCs and let in-flight calls finish, mirroring tonic graceful shutdown.

## AI Plan files
