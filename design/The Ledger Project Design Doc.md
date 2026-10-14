## Project Invariants
1. Each accepted operation MUST be zero-sum. The sum of DEBIT amounts equals the sum of CREDIT amounts. Operations with `amount == 0` are rejected. Persisted Journal and Book Records carry only a signed `amount`; the sign is `+` when the write aligns with the target account's natural direction and `−` when it opposes it.
2. Each Journal Record, Book Record, Operation Index Record MUST be composed of information written in its simplest binary format using the smallest possible data type for the range of its values.
3. If a book is flagged as disallowing overdrafts, the final net balance of an accepted operation MUST not put that book into overdraft.
4. A requested operation MUST only be accepted once it is written to the Operation Journal.
5. The `running_balance` of a book cover must always be equal to the sum of each entry starting from offset 0 of segment 1 up to the cover's declared `checkpoint_offset` and `checkpoint_segment`.
6. At the moment of startup, before handling any operation request, all operation journal records must be applied to their corresponding books.
7. No two operation journal records can have the same `operation_id`.
8. For the operation journal, any records N and N+1 must be arranged such that `N.operation_id + 1 == (N+1).operation_id`.
9. Similarly, for the book journal, any records N and N+1 must be arranged such that `N.operation_id <= (N+1).operation_id && ((N.target_segment < (N+1).target_segment) || (N.target_segment == (N+1).target_segment && N.target_offset < (N+1).target_offset))`.
10. At most one in-memory instance of each BookState can exist in the system. A BookState can only be evicted from memory if no in-flight operation is being processed across all steps.
11. Each Operation Record and Book Record MUST include a trailing 4-byte CRC32C checksum validating record integrity.

## Scoping Assumptions
- Any IO error while writing the journal (disk full or otherwise) must immediately shut down the system.
	- Full disk is unrecoverable without manual intervention.
	- The writer must not write entries out of order to preserve Invariant 8. Any IO error is unrecoverable.

## Phases
- [[Phase 1]]: Core functions
- [[Phase 2]]: Load Testing Harness
- [[Phase 3]]: Handler timeout, idempotency, and operation lookup
- [[Phase 4]]: Kafka variants