## Out of scope
- Handler timeout, idempotency, and operation lookup
- Record security and tampering prevention/detection
- Currency
- Archiving

## Tech Details
### Protobuf
```protobuf
syntax = "proto3";
package ledger;

enum AccountingType {
  ACCOUNTING_TYPE_UNSPECIFIED = 0;
  DEBIT = 1;
  CREDIT = 2;
}

enum BalanceType {
  BALANCE_TYPE_UNSPECIFIED = 0;
  AVAILABLE = 1;
  CURRENT = 2;
  HOLD = 3;
}

enum OperationErrorType {
  OPERATION_ERROR_TYPE_UNSPECIFIED = 0;
  ZERO_SUM_VIOLATION = 1;
  ENTRY_COUNT_EXCEEDED = 2;
  INVALID_LEDGER_CODE = 3;
  UNKNOWN_BOOK_ID = 4;
  AMOUNT_NOT_POSITIVE = 5;
  OVERDRAFT_REJECTED = 6;
}

message CreateAccountCommand {
  AccountingType accounting_type = 1;
  BalanceType balance_type = 2;
  bool allow_overdraft = 3;
}

message CreateAccountRequest {
  CreateAccountCommand command = 1;
}

message CreateAccountResponse {
  uint32 book_id = 1;
}

message OperationEntry {
  uint32 book_id = 1;
  AccountingType accounting_type = 2;
  int64 amount = 3;
  string ledger_code = 4;
}

message FinancialOperationCommand {
  repeated OperationEntry entries = 1;
}

message FinancialOperationRequest {
  FinancialOperationCommand command = 1;
}

message BookBalance {
  uint32 book_id = 1;
  string ending_balance = 2; // i128 (signed minor units), encoded as a base-10 decimal string with optional leading '-'
}

message OperationAccepted {
  uint64 operation_id = 1;
  string timestamp_ns = 2; // u128 epoch nanoseconds, encoded as a base-10 decimal string
  repeated BookBalance balances = 3;
}

message OperationError {
  OperationErrorType type = 1;
  optional uint32 offending_book_id = 2;
  optional uint32 offending_entry_index = 3; // zero-based index into FinancialOperationCommand.entries
}

message FinancialOperationResponse {
  oneof outcome {
    OperationAccepted accepted = 1;
    OperationError error = 2;
  }
}

// System-level failures (shutdown signal, IO error, internal channel closure)
// surface as gRPC status codes rather than in-band errors:
//   UNAVAILABLE on shutdown,
//   INTERNAL on IO error or unrecoverable internal state.
service LedgerService {
  rpc CreateAccount(CreateAccountRequest) returns (CreateAccountResponse);
  rpc ExecuteOperation(FinancialOperationRequest) returns (FinancialOperationResponse);
}
```
### Pseudocode
#### Assumptions
- Signals take precedent over other messages through ALL channels but do not interrupt processing. A shutdown signal will be processed immediately after the current message has been processed completely.
- `response_handle?` and similar semantics should be translated to the language's `Optional` semantics
#### On Startup
1. Initialize constant FIXED_SEGMENT_LENGTH as 64MiB
2. Start system tasks and initiate their channels:
	- Applier
	- Writer
	- Acceptor
	- Provisioner
	- Book Manager
3. Load the `operation.cov` described in [[#Operation Journal cover]]. Starting from the saved `current_segment=checkpoint_segment` and `current_offset=checkpoint_offset` traverse the operation journal. If the file does not exist, create it with `checkpoint_segment=1` and `checkpoint_offset=0` then system `operation_id_counter=1` as well as `journal.latest_segment=1` and `journal.latest_offset=0`. Send `PreProvisionOperationSegment(1, response_handle) and PreProvisionOperationSegment(2, response_handle)` to [[#Provisioner]] then await. Skip to #5.
	1. Load the `seg` file corresponding to `current_segment` and jump to `current_offset`
	2. Read the next `2 bytes` which represents the record length. If this is 0 or we've reached EOF, we've reached the end of the segment.
		1. Verify next segment by loading the `seg` file `current_segment+1` at `offset=0`. Read the next `2 bytes` which represents the record length, if this is 0 or if the segment does not exist, we've reached the end of the journal. Initialize the system `operation_id_counter` using the previous record's `operation_id+1` then exit the loop. Then initialize system `journal.latest_segment=current_segment` and `journal.latest_offset=current_offset`. If there's no previous record because we started traversing on an empty segment, traverse `journal.latest_segment-1` and start scanning records by reading `record_length` and skipping ahead until a `record_length==0` is found from which we re-read the last operation and initialize `operation_id_counter=record.operation_id+1`. If `journal.latest_segment-1` is `000...000.seg` , set `operation_id_counter=1` then skip to `#5`.
		2. Otherwise, set `current_segment=current_segment+1` and `current_offset=0` then loop back to `#3.2`
	3. Read the next `x bytes` where `x` is the `record length-record_length bytes` ([[#Operation Record]]). Collect distinct `operation_record_entry.target_book_id` then send a `LoadBook(target_book_id)` for each distinct book to [[#Book Manager]] and await its response. Then, after collecting the returned BookStates, send an `Apply([book_state,...], operation_record, response_handle)` to the [[#Applier]] via its channel asynchronously. Keep that record's `operation_id` in memory then loop back to `#3.2`
4. Wait for all `response_handle` to finish.
5. Load the file `book.id` and initialize the system `book_id counter`. If the file does not exist, set the counter to `1` and initialize the file with the value of `1u32`.
6. Initialize the system book registry which is a cache of `book_id->BookState`

#### Handlers
##### Create Account Request
1. decode gRPC `create account request`, send corresponding `Create(accounting_type, balance_type, allow_overdraft, response_handler)` message to [[#Book Manager]] via its channel
2. wait for response
3. Transform the result to the appropriate gRPC response:
	1. `CreateAccountResponse{book_id}`

##### Financial Operation Request
1. decode gRPC `operation request`, validate the request and check its entries. On any validation failure, return a `FinancialOperationResponse{error: OperationError{...}}` with the listed `OperationErrorType` and populated fields; the request must not reach the [[#Acceptor]]:
	1. All the `book_id` < system book_id_counter. On failure: `UNKNOWN_BOOK_ID` with `offending_book_id` set to the unknown id.
	2. All `amount` > 0. On failure: `AMOUNT_NOT_POSITIVE` with `offending_book_id` and `offending_entry_index` set to the first offending entry.
	3. All `ledger_code` are in ASCII and have length <= 8. On failure: `INVALID_LEDGER_CODE` with `offending_book_id` and `offending_entry_index` set to the first offending entry.
	4. The number of entries must not exceed 255. On failure: `ENTRY_COUNT_EXCEEDED` with no offending fields.
	5. The sum of all amounts such that DEBITS are negative and CREDITS are positive must be zero. On failure: `ZERO_SUM_VIOLATION` with no offending fields. Note that this rule is only for validation. Actual signage is identified by comparing the `accounting_type` of the Book and the operation entries.
2. Preprocess each operation entry as follows:
	1. Try to fetch each `book_id` from the system book registry. Keep track if which books are missing.
	2. For each missing book, send a `LoadBook(book_id)` to the [[#Book Manager]] via its channel asynchronously then wait for the resulting `BookState` of each. `LoadBook` always returns `Ok(BookState)` for any `book_id` that passed validation step 1.1; any IO failure inside the [[#Book Manager]] triggers a system shutdown and surfaces here as Book Manager channel closure, handled by the [[#System Failure]] path below.
	3. Once all books have been loaded, each entry's amount must be transformed to its `signed_amount` equivalent. If the entry's `accounting_type` does not match the book's `accounting_type` (i.e. DEBIT vs CREDIT), then negate the amount. Accumulate this amount for the corresponding `book_id` as `incoming_total`
	4. Transform the `ledger_code` string to its equivalent `byte_array`, left padded with whitespace characters.
	5. Collect an `op[]` array which contains `(signed_amount, ledger_array_bytes)`
	6. Group the entries with their respective book such that the preprocessing result is `[{book_state, incoming_total, op[]}]`
3. Send an `Accept(preprocessing_result, result_handle)` to [[#Acceptor]] via its channel asynchronously then wait for the result.
4. Transform the result to the appropriate gRPC response:
	- `Acceptor.Accepted(operation_id, timestamp_ns, ending_balances)` maps to `FinancialOperationResponse{accepted: OperationAccepted{operation_id, timestamp_ns, balances=ending_balances}}`.
	- `Acceptor.Reject(book_id)` maps to `FinancialOperationResponse{error: OperationError{OVERDRAFT_REJECTED, offending_book_id=book_id}}`.
##### System Failure 
- when the book manager channel is no longer accepting messages
	- then the handler must immediately return a system failure
- the acceptor channel is no longer accepting messages
	- then the handler must immediately return a system failure

#### Book Manager
##### Create(accounting_type, balance_type, allow_overdraft, response_handle)
1. load `book_id_counter` . 
2. Check `book_covers/<first 6 digits of book_id_counter>.cov : Line <last 4 digits of book_id_counter>` if an entry exists. If occupied, increment `book_id_counter` by 1 again since this means there was a system crash between creating the book and #5. Repeat until an empty line is found. Then, initialize with values for [[#Book Journal cover]] as `book.running_balance=0`, `book.accounting_type=accounting_type`, `book.balance_type=balance_type`, `book.allow_overdraft=allow_overdraft`, `book.checkpoint_segment=1`, `book.checkpoint_offset=0`. If file does not exist, create `book_covers/<first 6 digits of book_id_counter>.cov`
3. send `PreProvisionBookSegment(book_id_counter, 1, response_handle) and PreProvisionBookSegment(book_id_counter, 2, response_handle)` to [[#Provisioner]] via its channel then await both.
4. Write  `book_id_counter+1`, then fsync. 
5. send `LoadBook(book_id_counter++, response_handle)` to self
##### LoadBook(book_id, response_handle)
1. Read the `book_covers/<first 6 digits of book_id>.cov : Line <last 4 digits of book_id>` described in [[#Book Journal cover]]. Starting from the saved `current_segment=checkpoint_segment` and `current_offset=checkpoint_offset` traverse the book journal. Inititalize the `BookState` as `book.projected_balance=running_balance`, `book.accounting_type=accounting_type`, `book.balance_type=balance_type`, `book.allow_overdraft=allow_overdraft`, `book.latest_segment=1`, `book.latest_offset=0` and `book.id=book_id`.
	1. Load the `seg` file corresponding to `current_segment` and jump to `current_offset`
	2. Read the next `2 bytes` which represents the record length. If this is 0 or we've reached EOF, we've reached the end of the segment.
		1. Verify next segment by loading the `seg` file `current_segment+1` at `offset=0`. Read the next `2 bytes` which represents the record length, if this is 0 or if the segment does not exist, we've reached the end of the journal. Initialize `book.latest_segment=current_segment`, `book.latest_offset=current_offset`.
		2. Otherwise, set `current_segment=current_segment+1` and `current_offset=0` then loop back to `#1.2`. 
	3. Read the next `x bytes` where `x` is the record length ([[#Book Record]]) then add the record's `amount` to `projected_balance` then loop back to `#1.2` .
2. Insert the `BookState` into the book registry
3. if `response_handle` was provided, send an `Ok(BookState)`
##### Periodic Registry Eviction
1. Every `system.cleanup_interval`, scan the registry for any book whose reference count is zero and evict them from the registry
	- In Rust, this can be done with `Weak<BookState>` on the registry and checking `weak_ref.strong_count() == 0` 
	- In Kotlin, this can be done with `WeakReference<BookState>` on the registry and checking `weak_ref.get() == null` 
##### when `shutdown signal` is received
1. close the book manager channel and discard any incoming message.
##### System Failure 
- when the provisioner channel is no longer accepting messages
	- then the handler must immediately return a system failure
- when the book manager encounters any IO failure, trigger a system shutdown.
	- This either means full disk or missing file which shouldn't be possible because book_ids are validated before loading.

#### Acceptor
##### Accept(preprocessing_result, result_handle)
1. initialize `write_batch` with size `write_batch_capacity` if not yet present (typically on the first message after startup).
2. Validation Pass. Iterate over each `{book_state, incoming_total, op[]}` of preprocessing_result and validate `book_state.allow_overdraft || book_state.projected_balance + incoming_total >= 0 `
	1. If any book fails the validation, send a `Reject(book_state.id)` through `result_handle`, stop processing the message (short circuit).
3. Commit Pass. 
	1. Prepare `timestamp_ns=Timestamp.now().nanoseconds`, `entry_count=sum(op[].size)`, and `operation_id=operation_id counter++`
	2. Iterate over each `{book_state, incoming_total, op[]}` of preprocessing_result and update `book_state.projected_balance += incoming_total`
		1. iterate over each element of `op[]`
			1. if `FIXED_BOOK_RECORD_SIZE_INCLUDING_RECORD_LENGTH + book_state.latest_offset > FIXED_SEGMENT_LENGTH`, then set `book_state.latest_offset=0`,`book_state.latest_segment++` and send a `PreProvisionBookSegment(book_state.id, book_state.latest_segment)` to [[#Provisioner]] via its channel asynchronously as a preemptive provisioning which will be reinforced by [[#Applier]].
			2. prepare `target_book_id=book_state.id`, `target_segment=book_state.latest_segment`, `target_offset=book_state.latest_offset`, `amount=element.amount`, and `ledger_code=element.ledger_code`
			3. set `book_state.latest_offset+= FIXED_BOOK_RECORD_SIZE_INCLUDING_RECORD_LENGTH`
			4. Append the prepared bytes to a temporary `instructions` buffer
		2. collect a map of `book_state.id->ending_balance` 
	3. Prepare `record_length=size(timestamp_ns)+size(entry_count)+size(operation_id)+size(instructions)+2 bytes (record length)` and the bytes after appending all data in accordance with [[#Operation Record]]
	4. Prepare a `WriteOperation([book_state,...], operation_record, result_handle, ending_balances)` and append to the `write_batch`
	5. if `write_batch` has exactly 1 `WriteOperation`, start the `flush_timer` for `system.flush_timeout_millis`
	6. if `write_batch` is full, swap the `write_batch` with a new empty array of the same capacity and send `WriteBatch(write_batch)` to [[#Writer]] through its channel. Cancel `flush_timer`.
##### when `flush_timer` is up and write_batch is not empty
1. swap the `write_batch` with a new empty array of the same capacity and send `WriteBatch(write_batch)` to [[#Writer]] through its channel

##### when `shutdown signal` is received
1. close the acceptor channel
2. drain the acceptor channel and send a `Failure(system_shutdown)` through `response_handle` of each
3. drain the `write_batch` and send a `Failure(system_shutdown)` through `response_handle` of each

##### System Failure 
- when the provisioner is no longer accepting messages, do nothing. It's not a critical path for Acceptor.
- when the writer channel is no longer accepting messages
	1. close the incoming acceptor channel
	2. drain the acceptor channel and send a `Failure(system_shutdown)` through `response_handle` of each
	3. drain the `write_batch` and send a `Failure(system_shutdown)` through `response_handle` of each

#### Writer
##### WriteBatch(write_batch)
1. Iterate over each `{book_states[], operation_record, result_handle, ending_balances}` of write_batch
	1. if `size(disk_buffer) + size(operation_record) + journal.latest_offset >= FIXED_SEGMENT_LENGTH`, then 
		1. write `disk_buffer` to `operation_segments/<journal.latest_segment>.seg` starting at `journal.latest_offset`, then fdatasync the file
		2. replace `disk_buffer` with an empty buffer.
		3. `journal.latest_offset=0`,`journal.latest_segment++`
		4. send a `PreProvisionOperationSegment(journal.latest_segment+1)` to [[#Provisioner]] via its channel as a preemptive provisioning which will be reinforced by [[#Applier]]
	2. append `operation_record` to the `disk_buffer`
2. track `flush_size=disk_buffer.size` for later, write `disk_buffer` to `operation_segments/<journal.latest_segment>.seg` starting at `journal.latest_offset`, then fdatasync the file
3. `journal.latest_offset+=flush_size`
4. Acknowledge handlers. Iterate over each `{operation_record, result_handle, ending_balances}` of write_batch
	1. send `Accepted(operation_record.operation_id,operation_record.timestamp_ns,ending_balances)` through `result_handle`
	2. send `Apply([book_state,...], operation_record)` to [[#Applier]] via its channel. `[book_states]` is kept to keep their optimistic cache entry alive until the operation is applied.
##### when `shutdown signal` is received
1. close the writer channel
2. drain the writer channel and send a `Failure(system_shutdown)` through `response_handle` of each

##### System Failure
- when the writer encounters any form of IO error
	1. close the incoming writer channel
	2. drain the writer channel and send a `Failure(system_shutdown)` through `response_handle` of each
	3. `write_batch` is discarded to avoid the risk of incomplete writes. Startup recovery will handle this.

#### Applier
##### Apply(book_states[], operation_record, response_handle)
1. Validate parity using the Applier's in-memory `checkpoint_segment` and `checkpoint_offset`. The Applier holds these in memory for its lifetime after initializing them from `operation.cov` on its first message and persisting them back to disk in step 6.
2. Set `temp_offset=checkpoint_offset` and `temp_segment=checkpoint_segment` then compute `temp_offset + operation_record.record_length`
	1. if it's `>= FIXED_SEGMENT_LENGTH`, set `temp_segment=checkpoint_segment+1` and `temp_offset=0`
	2. Read `temp_offset to temp_offset+operation_record.record_length` in `operation_segments/<temp_segment>.seg` and verify that it contains the exact same `operation_record` if not, trigger a system shutdown.
3. check if `book_segments/<target_book_id first 6 digits>/<target_book_id>-<target_segment>.seg` exists for all `operation_entry[]`, if any does not exist, send a `PreProvisionBookSegment(target_book_id, missing_segment, response_handle)` to [[#Provisioner]] via its channel and wait for completion.
4. Open all `book_segments/<target_book_id first 6 digits>/<target_book_id>-<target_segment>.seg` for writing.
5. Iterate over the entries of `operation_record` 
	1. Build the book record: copy `operation_id` and `timestamp_ns` from the `operation_record`, copy `amount` and `ledger_code` from the entry, prepend computed `record_length`. On `book_segments/<target_book_id first 6 digits>/<target_book_id>-<target_segment>.seg`, write the record at `target_offset`, then fdatasync the file.
6. Set `checkpoint_offset=temp_offset+operation_record.record_length` and `checkpoint_segment=temp_segment`. Write `checkpoint_segment` and `checkpoint_offset` to `operation.cov`, then fsync the file.
7. For each `book_state` in `book_states[]`
	1. Read the book cover at `book_covers/<first 6 digits of book_state.id>.cov : Line <last 4 digits of book_state.id>`. Let `book_checkpoint_segment` and `book_checkpoint_offset` denote the cover's `checkpoint_segment` and `checkpoint_offset` (named distinctly from the operation cover's same-named fields used in steps 1-6). Starting from `current_segment=book_checkpoint_segment` and `current_offset=book_checkpoint_offset` traverse the book journal. Initialize `rollup_balance` from the cover's `running_balance`.
		1. Load the `seg` file corresponding to `current_segment` and jump to `current_offset`
		2. Read the next `2 bytes` which represents the record length. If this is 0 or we've reached EOF, we've reached the end of the segment.
			1. Verify next segment by loading the `seg` file `current_segment+1` at `offset=0`. Read the next `2 bytes` which represents the record length, if this is 0 or if the segment does not exist, we've reached the end of the journal. Set `book_checkpoint_segment=current_segment`, `book_checkpoint_offset=current_offset`.
			2. Otherwise, set `current_segment=current_segment+1` and `current_offset=0` then loop back to `#7.1.2` 
		3. Read the next `x bytes` where `x` is the record length ([[#Book Record]]) then add the record's `amount` to `rollup_balance` then loop back to `#7.1.2` 
	2. Write `book_checkpoint_segment` (as `checkpoint_segment`), `book_checkpoint_offset` (as `checkpoint_offset`), and `rollup_balance` (as `running_balance`) to `book_covers/<first 6 digits of book_state.id>.cov : Line <last 4 digits of book_state.id>` while keeping `accounting_type`, `balance_type`, and `allow_overdraft` intact, then fsync the file.
8. if `response_handle` was provided, send an `Ok`
##### when `shutdown signal` is received
1. stop processing `Apply()` messages immediately after processing the current one. Any inflight messages are discarded. Startup recovery will handle them.

##### System Failure
- when the applier encounters any form of IO error
	1. stop processing `Apply()` messages immediately after processing the current one
	2. trigger a system shutdown
#### Provisioner
##### PreProvisionOperationSegment(segment_number, response_handle?)
1. pre-allocate a `FIXED_SEGMENT_LENGTH` sized, zero-filled file in `operation_segments/<segment_number>.seg` if the file does not exist. Otherwise, do nothing.
2. if `response_handle` was provided, send an `Ok`
##### PreProvisionBookSegment(book_id, segment_number, response_handle?)
1. pre-allocate a `FIXED_SEGMENT_LENGTH` sized, zero-filled file in `book_segments/<book_id first 6 digits>/<book_id>-<segment_number>.seg`  if the file does not exist. Otherwise, do nothing.
2. if `response_handle` was provided, send an `Ok`
##### when `shutdown signal` is received
1. close the provisioner channel and discard any incoming message.
##### System Failure
- when the provisioner encounters any form of IO error
	1. close the incoming provisioner channel.
	2. trigger a system shutdown

### Persistent Primitives

### Journals

The whole persistence layer of the ledger is structured around "event" sourcing journals. Journals have 3 main components:
1. A **cover** file which is the first contact point and contains metadata that's required to traverse the segments.
2. Multiple **segment** files which are pre-allocated append-only files that that divide the entire journal sequence into manageable blocks. Segments contain sequenced records.
3. An **record** is a unit of data that describes a certain action that is done on the state of the account. Applying each record in the final state of whichever domain entity the journal represents.

Journals come in two variants:
- **Book Journals** are actual DEBIT/CREDIT records performed against the account. Traversing this gives us the current balance of that particular book.
- **Operation Journals** are more like commit logs whose records contain literal instructions on what offset of which segment of which book to write a sequence of bytes into. Each record is a collection of multiple write instructions that make up one atomic transaction whose successful write signals the system to acknowledge the request handler that initiated it. Traversing the Operation Journal gives us the current state of ALL books in the system.
### Covers
- All covers have a `checkpoint_offset` and `checkpoint_segment` which describes the coordinates for which the rollup has last stopped.
	- For Books, this describes up to what record the cover's `running_balance` was based on
	- For Operations, this describes up to what record has been applied to their respective books

### Records
- All records have a a header section which describes the `record_length` that allows readers to quickly skip over records.

### Runtime Models
#### BookState

| Field               | type      | Notes                                                                                                                            |
| ------------------- | --------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `id`                | `u32`     |                                                                                                                                  |
| `projected_balance` | `i128`    | optimistic projection of what would be the running balance of the corresponding cover once all in-flight operations are applied. |
| `accounting_type`   | `enum`    |                                                                                                                                  |
| `balance_type`      | `enum`    |                                                                                                                                  |
| `allow_overdraft`   | `boolean` |                                                                                                                                  |
| `latest_segment`    | `u32`     | Identified during [[#LoadBook(book_id)]]. Marks where the next operation would be appended. Incremented by the [[#Acceptor]]     |
| `latest_offset`     | `u32`     | Identified during [[#LoadBook(book_id)]]. Marks where the next operation would be appended. Incremented by the [[#Acceptor]]     |
#### System

| Field                     | type  | Notes                                                                                                                        |
| ------------------------- | ----- | ---------------------------------------------------------------------------------------------------------------------------- |
| `journal.latest_offset`   | `u32` | Identified during [[#On Startup]]. Marks where the next operation would be appended. Incremented by the [[#Writer]]          |
| `journal.latest_segment`  | `u32` | Identified during [[#On Startup]]. Marks where the next operation would be appended. Incremented by the [[#Writer]]          |
| `cleanup_interval_millis` | `u32` | Eviction scan interval of the book registry. Default to `60000`                                                              |
| `flush_timeout_millis`    | `u32` | Duration from the first operation when the `write_batch` will be sent to the [[#Writer]] from [[#Acceptor]]. Default to `10` |
| `book_id_counter`         | `u32` | Identified during [[#On Startup]]. Incremented by the [[#Book Manager]]                                                      |
| `operation_id_counter`    | `u64` | Identified during [[#On Startup]]. Incremented by the [[#Acceptor]]                                                          |
| `FIXED_SEGMENT_LENGTH`    | `u32` | Default to `64MiB`                                                                                                           |
| `write_batch_capacity`    | `u16` | Default to `20`                                                                                                              |

### Persistence Models
### Operation Journal
cover filename: `operation.cov`
segment filename: `operation_segments/<u32 segment_number>.seg`
#### Operation Journal cover

| Field                | type  | Notes                                                                                                                                                            |     |
| -------------------- | ----- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | --- |
| `checkpoint_segment` | `u32` | Segment of the furthest position whose records have been verified as applied to their target books.                                                              |     |
| `checkpoint_offset`  | `u32` | Offset after the furthest position whose records have been verified as applied to their target books. This describes where to start reading for the next record. |     |
#### Operation Record 

| Field           | type   | Notes                                                                         |
| --------------- | ------ | ----------------------------------------------------------------------------- |
| `record_length` | `u16`  | Total bytes of record including the `record_length`.                          |
| `entry_count`   | `u8`   | Number of Write Instructions in the body. Runtime cap enforced at acceptance. |
| `operation_id`  | `u64`  | Journal sequence assigned at acceptance.                                      |
| `timestamp_ns`  | `u128` | Epoch nanoseconds at acceptance.                                              |
| `entries`       | *      | multiple entries                                                              |

#### Operation Entry 

| Field            | type      | Notes                                                                                                              |
| ---------------- | --------- | ------------------------------------------------------------------------------------------------------------------ |
| `target_book_id` | `u32`     | engine-assigned dense sequence                                                                                     |
| `target_segment` | `u32`     | book segment to write on.                                                                                          |
| `target_offset`  | `u32`     | segment offset to write on.                                                                                        |
| `amount`         | `i64`     | Minor units. Positive when the write aligns with the target book's `accounting_type`; negative when it opposes it. |
| `ledger_code`    | `[u8; 8]` | Fixed-width code carried for observability and reconciliation.                                                     |


### Book Journal
cover filename and line: `book_covers/<first 6 digits of book_state.id>.cov : Line <last 4 digits of book_state.id>`
segment filename: `book_segments/<book_id first 6 digits>/<u32 book_id>-<u32 segment_number>.seg`

#### Book Journal cover

| Field                | type             | Notes                                                                                                                                |
| -------------------- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `balance_type`       | enum, `repr(u8)` | This book's balance type. `AVAILABLE`, `CURRENT`, `HOLD`                                                                             |
| `accounting_type`    | enum, `repr(u8)` | Natural direction of this book. Fixed at creation. Drives DR/CR-to-signed translation at pre-acceptance. `DEBIT`, `CREDIT`           |
| `allow_overdraft`    | `bool`           | Whether this book may be driven negative by an accepted Operation. Fixed at creation.                                                |
| `running_balance`    | `i128`           | Signed minor units, natural-direction convention. Must be updated through a overflow-safe addition.                                  |
| `checkpoint_segment` | `u32`            | Segment of the last Book Record position folded into `running_balance`.                                                              |
| `checkpoint_offset`  | `u32`            | Offset after the last Book Record position folded into `running_balance`. This describes where to start reading for the next record. |

#### Book Record

| Field           | type      | Notes                                                                                  |
| --------------- | --------- | -------------------------------------------------------------------------------------- |
| `record_length` | `u16`      | Total bytes of record including the `record_length`.                                   |
| `operation_id`  | `u64`     | Journal sequence assigned at acceptance.                                               |
| `timestamp_ns`  | `u128`    | Epoch nanoseconds at acceptance.                                                       |
| `amount`        | `i64`     | Minor units. Sign already encodes direction relative to this book's `accounting_type`. |
| `ledger_code`   | `[u8; 8]` | Fixed-width code.                                                                      |


### Directory Structure
### Live Data Layout
```
data/ 
├── book.id
├── book_covers/ 
│   ├── 000...003.cov <-- books 000...030...000 to 000...039...999
│   └── 000...004.cov <-- books 000...040...000 to 000...049...999
├── book_segments/ 
│   ├── 000...003/
│   └── 000...004/
│       ├── 000...004...001-000...001.seg
│       ├── 000...004...002-000...001.seg   
│       └── 000...004...002-000...002.seg
├── operation.cov
└── operation_segments/ 
    ├── 000...002/
    └── 000...003/
        ├── 000...003...001.seg
        ├── 000...003...002.seg   
        └── 000...003...003.seg        

```

### Archive Data Layout
```
archive/ 
├── book_segments/ 
│   ├── 000...001/
│   └── 000...002/
│       ├── 000...002...001-000...001.seg
│       ├── 000...002...002-000...001.seg   
│       └── 000...002...002-000...002.seg
└── operation_segments/ 
	├── 000...001/
    └── 000...002/
        ├── 000...002...001.seg
        ├── 000...002...002.seg   
        └── 000...002...003.seg  
```

