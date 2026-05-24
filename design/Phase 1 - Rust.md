## Implementation Guidelines
- Use `biased;` for `select!` so we can explictly control message priorities in tasks. Typically, shutdown signals go first.
    - for the `Acceptor`, `Accept` takes precedence over the `flush_timer`
    - for the `Applier`, provisioning book segments takes precedence over operation segments so operations are backpressured.

## AI Plan files
- [[Phase 1 - Rust - gRPC Scaffolding]]
- [[Phase 1 - Rust - Actor Stubs]]