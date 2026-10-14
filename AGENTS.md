# The Ledger Project

- Non-production ledger system implemented in Rust as a proof of concept.
- **Use simple and concise language when writing notes.** Assume that the reader is a software engineer with mostly web-development and FinTech experience with limited familiarity with (but is trying to learn) systems-development concepts and low-level jargon.
- Avoid any sentence structures that set up and then negate or expand beyond expectations (like "X isn't just about Y" or "X is more than just Y" or "X goes beyond Y" or "It's X, not Y"). Avoid these "contrastive structure" in prose at all costs. When encountering content in files, sanitize them immediately but using direct affirmative sentences. If negation is necessary and unavoidable, elaborate and be specific about it.

## Folder Structure

- `design`: An Obsidian vault containing notes and specification documents.
- `rust`: Rust implementation source code.
- `k6`: Load testing configuration.
- `proto`: Protobuf schema files.

## Plans

- All plan files must be placed in the `design` directory and linked to their corresponding `Phase <n> - Rust.md`.

## Git

- Do not directly commit or push any code to git. This applies to the main agent and any spawned subagent.

## Rust
- Use `biased;` for `select!` so we can explictly control message priorities in tasks. Typically, shutdown signals go first.
    - for the `Acceptor`, `Accept` takes precedence over the `flush_timer`
    - for the `Applier`, provisioning book segments takes precedence over operation segments so operations are backpressured.
- Use `tokio` for the runtime
- Use `zerocopy` to define structures to represent file entries
- Use `thiserror` for error translation and propagation
- Use `tonic` (0.14) for receiving gRPC requests
- Use `prost` (0.14) for protobuf message types
- For protobuf codegen, use `tonic-prost-build` as a build dependency. In tonic 0.14 the prost integration was extracted out of `tonic-build` into a separate crate, so the build script must call `tonic_prost_build::configure()` (not `tonic_build::configure()`). At runtime, also depend on `tonic-prost` (0.14) for the prost codec.
- `tonic::async_trait` and `tonic::include_proto!` are still re-exported from the `tonic` crate, so handler code does not need to import the prost-build crates directly.
- Handle all error results instead of calling `unwrap` to avoid system panic
- Prefer moving values rather than borrowing or copy/cloning whenever applicable and appropriate.
- Do not directly call `drop()`, instead use block scoping and let Rust drop things as they go out of scope
- Do not automatically add macros to silence clippy findings.
