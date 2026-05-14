## Guidelines
- Use `tokio` for the runtime
- Use `zerocopy` to define structures to represent file entries
- Use `thiserror` for error translation and propagation
- Use `tonic` for receiving gRPC requests
- Use `prost` for codegen of `.proto` files
- Handle all error results instead of calling `unwrap` to avoid system panic
- Prefer moving values rather than borrowing or copy/cloning whenever applicable and appropriate.
- Do not directly call `drop()`, instead use block scoping and let Rust drop things as they go out of scope

## AI Plan files
- [[2026-05-14-protobuf-definitions]]