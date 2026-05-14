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

enum AccountType {
  ACCOUNT_TYPE_UNSPECIFIED = 0;
  DEBIT = 1;
  CREDIT = 2;
}

enum BalanceType {
  BALANCE_TYPE_UNSPECIFIED = 0;
  AVAILABLE = 1;
  CURRENT = 2;
  HOLD = 3;
}

message CreateAccountCommand {
  AccountType accounting_type = 1;
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
  AccountType accounting_type = 2;
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
  int64 ending_balance = 2;
}

message FinancialOperationResponse {
  uint64 operation_id = 1;
  uint64 timestamp_ns = 2;
  repeated BookBalance balances = 3;
}

service LedgerService {
  rpc CreateAccount(CreateAccountRequest) returns (CreateAccountResponse);
  rpc ExecuteOperation(FinancialOperationRequest) returns (FinancialOperationResponse);
}
```
### Pseudocode
#### On Startup
1. Initialize constant FIXED_SEGMENT_LENGTH as 64MiB
2. Start system tasks and initiate their channels:
	- Applier
	- Writer
	- Acceptor
3. Load the `operation.cov` described in [[#Operation Journal cover]]. Starting from the saved `current_segment=checkpoint_segment` and `current_offset=checkpoint_offset` traverse the operation journal
	1. Load the `seg` file corresponding to `current_segment` and jump to `current_offset`
	2. Read the next `2 bytes` which represents the record length. If this is 0 or we've reached EOF, we've reached the end of the segment.
		1. Verify next segment by loading the `seg` file `current_segment+1` at `offset=0`. Read the next `2 bytes` which represents the record length, if this is 0, we've reached the end of the journal. Initialize the system `operation_id counter` using the previous record's `operation_id+1` then exit the loop. Then initialize system `journal.latest_segment=current_segment` and `journal.latest_offset=current_offset`. If there's no previous record because we started traversing on an empty segment, traverse `journal.latest_segment-1` and skip over records until we get to the final segment record.
		2. Otherwise, set `current_segment=current_segment+1` and `current_offset=0` then loop back to `#2.2`
	3. Read the next `x bytes` where `x` is the `record length-record_length bytes` ([[#Operation Record]]) and send an `Apply(operation_record)` to the [[#Applier]] via its channel asynchronously. Keep that record's `operation_id` in memory then loop back to `#2.2`
4. Send `Ping` to the Applier via its channel and wait for a response to indicate that all journal records have been applied.
5. Load the file `book.id` and initialize the system `book_id counter`
6. Initialize the system book registry which is a cache of `book_id->BookState`

#### Handlers
##### Create Account Request
1. decode gRPC `create account request`, send corresponding `Create(account_type, balance_type, allow_overdraft)` message to [[#Book Registry Actor]] via its channel
2. wait for response

##### Financial Operation Request
1. decode gRPC `operation request`, validate the request and check its entries for the following:
	1. All the `book_id` <= system book_id counter
	2. All `amount` > 0
	3. All `ledger_code` are in ASCII and have length <= 8
	4. The number of entries must not exceed 255
	5. The sum of all amounts such that DEBITS are negative and CREDITS are positive must be zero
2. Preprocess each operation entry as follows:
	1. Try to fetch each `book_id` from the system book registry. Keep track if which books are missing.
	2. For each missing book, send a `LoadBook(book_id)` to the [[#Book Registry Actor]] via its channel asynchronously then wait for the resulting `BookState` of each
	3. Once all books have been loaded, each entry's amount must be transformed to its `signed_amount` equivalent. If the entry's `accounting_type` does not match the book's `accounting_type` (i.e. DEBIT vs CREDIT), then negate the amount. Accumulate this amount for the corresponding `book_id` as `incoming_total`
	4. Transform the `ledger_code` string to its equivalent `byte_array`
	5. Collect an `op[]` array which contains `(signed_amount, ledger_array_bytes)`
	6. Group the entries with their respective book such that the preprocessing result is `[{book_state, incoming_total, op[]}]`
3. Send an `Accept(preprocessing_result, result_handle)` to [[#Acceptor]] via its channel asynchronously then wait for the result.
4. transform the result to the appropriate response.
##### System Failure 
- when the book registry channel is no longer accepting messages
	- then the handler must immediately return a system failure
- the acceptor channel is no longer accepting messages
	- then the handler must immediately return a system failure

#### Book Registry Actor
##### Create(accounting_type, balance_type, allow_overdraft)
1. load `book.id` (create 8-byte file with value 1 if it does not exist).
2. initialize `book_id` as this value. Increment file value by 1
3. create `<book_id>.cov` and initialize with values for [[#Book Journal cover]] 
4. send `PreProvisionBookSegment(book_id, 1) and PreProvisionBookSegment(book_id, 2)` to [[#Provisioner]] via its channel
5. respond to handler
##### LoadBook(book_id)
1. Load the `book_covers/<book_id>.cov` described in [[#Book Journal cover]]. Starting from the saved `current_segment=checkpoint_segment` and `current_offset=checkpoint_offset` traverse the book journal. Inititalize the `BookState` as `book.projected_balance=running_balance`, `book.account_type=account_type`, `book.balance_type=balance_type`, `book.allow_overdraft=allow_overdraft`, and `book.id=book_id`.
	1. Load the `seg` file corresponding to `current_segment` and jump to `current_offset`
	2. Read the next `2 bytes` which represents the record length. If this is 0 or we've reached EOF, we've reached the end of the segment.
		1. Verify next segment by loading the `seg` file `current_segment+1` at `offset=0`. Read the next `2 bytes` which represents the record length, if this is 0, we've reached the end of the journal. Initialize `book.latest_segment=current_segment`, `book.latest_offset=current_offset`.
		2. Otherwise, set `current_segment=current_segment+1` and `current_offset=0` then loop back to `#1.2` 
	3. Read the next `x bytes` where `x` is the record length ([[#Book Record]]) then add the record's `amount` to `projected_balance` then loop back to `#1.2` 
2. Insert the `BookState` into the book registry
##### Periodic Registry Eviction
1. Every `system.cleanup_interval`, scan the registry for any book whose reference count is zero and evict them from the registry
	- In Rust, this can be done with `Weak<BookState>` on the registry and checking `weak_ref.strong_count() > 0` 
	- In Kotlin, this can be done with `WeakReference<BookState>` on the registry and checking `weak_ref.get() == null 
##### System Failure 
- when the book registry channel is no longer accepting messages
	- then the handler must immediately return a system failure
- the acceptor channel is no longer accepting messages
	- then the handler must immediately return a system failure

#### Acceptor
##### Accept(preprocessing_result, result_handle)
1. Validation Pass. Iterate over each `{book_state, incoming_total, op[]}` of preprocessing_result and validate `book_state.allow_overdraft || book_state.projected_balance + incoming_total >= 0 `
	1. If any book fails the validation, send a `Reject(book_state.id)` through `result_handle`, stop processing the message
	2. collect a map of `book_id->ending_balance` 
2. Commit Pass. 
	1. Prepare `timestamp_ns=Timestamp.now().nanoseconds`, `entry_count=instructions.size`, and `operation_id=operation_id counter++`
	2. Iterate over each `{book_state, incoming_total, op[]}` of preprocessing_result and update `book_state.projected_balance += incoming_total`
		1. iterate over each element of `op[]`
			1. if `FIXED_BOOK_RECORD_SIZE_INCLUDING_RECORD_LENGTH + book_state.latest_offset > FIXED_SEGMENT_LENGTH`, then set `book_state.latest_offset=0`,`book_state.latest_segment++` and send a `PreProvisionBookSegment(book_state.id, book_state.latest_segment)` to [[#Provisioner]] via its channel asynchronously.
			2. prepare `record_length=FIXED_BOOK_RECORD_SIZE_INCLUDING_RECORD_LENGTH` , `target_book_id=book_state.id`, `target_segment=book_state.latest_segment`, `target_offset=book_state.latest_offset`, `amount=element.amount`, and `ledger_code=element.ledger_code`
			3. set `book_state.latest_offset+= FIXED_BOOK_RECORD_SIZE_INCLUDING_RECORD_LENGTH`
			4. Append the prepared bytes to a temporary `instructions` buffer
	3. Prepare `record_length=size(timestamp_ns)+size(entry_count)+size(operation_id)+size(instructions)` and the bytes appending all data in accordance with [[#Operation Record]]
	4. Prepare a `WriteOperation(book_state, operation_record, result_handle, ending_balances)` and append to the `write_batch`
	5. if `write_batch` has exactly 1 `WriteOperation`, start the `flush_timer` for `system.flush_timeout_millis`
	6. if `write_batch` is full, swap the `write_batch` with a new empty array of the same capacity and send `WriteBatch(write_batch)` to [[#Writer]] through its channel
##### when `flush_timer` is up
1. swap the `write_batch` with a new empty array of the same capacity and send `WriteBatch(write_batch)` to [[#Writer]] through its channel

##### when `shutdown signal` is received
1. close the acceptor channel
2. drain the acceptor channel and send a `Failure(system_shutdown)` through `response_handle` of each
3. drain the `write_batch` and send a `Failure(system_shutdown)` through `response_handle` of each
##### System Failure 
- when the writer channel is no longer accepting messages
	1. close the incoming acceptor channel
	2. drain the acceptor channel and send a `Failure(system_shutdown)` through `response_handle` of each

#### Writer
##### WriteBatch(write_batch)
1. Iterate over each `{book_state, operation_record, result_handle, ending_balances}` of write_batch
	1. if `size(disk_buffer) + size(operation_record) + journal.latest_offset > FIXED_SEGMENT_LENGTH`, then 
		1. flush `disk_buffer` to the disk through `operation_segments/<journal.latest_segment>.seg` starting at  `journal.latest_offset`
		2. replace `disk_buffer` with an empty buffer.
		3. `journal.latest_offset=0`,`journal.latest_segment++`
		4. send a `PreProvisionOperationSegment(journal.latest_segment+1)` to [[#Provisioner]] via its channel
	2. append `operation_record` to the `disk_buffer`
2. track `flush_size=disk_buffer.size` for later, flush `disk_buffer` to the disk through `operation_segments/<journal.latest_segment>.seg` starting at  `journal.latest_offset`
3. `journal.latest_offset+=flush_size`
4. Acknowledge handlers. Iterate over each `{operation_record, result_handle, ending_balances}` of write_batch
	1. send `Accepted(ending_balances)` through `result_handle`
	2. collect `operation_record` into a `apply_batch` map of `book_id->{book_state, [operation_record...]}`
##### Periodically dispatch Apply() tasks
1. Every `system.apply_dispatch_interval`, swap `apply_batch` with an empty map then transform each entry to an `Apply(book_state, [operation_record...])` then send the message to [[#Applier]] via its channel
##### when `shutdown signal` is received
1. close the writer channel
2. drain the writer channel and send a `Failure(system_shutdown)` through `response_handle` of each

##### System Failure
- when the writer encounters any form of IO error
	1. close the incoming writer channel
	2. drain the writer channel and send a `Failure(system_shutdown)` through `response_handle` of each

#### Applier
##### Apply(book_state, operation_record[])
1. check if `book_segments/<book_id first 6 digits>/<book_state.id>-<target_segment>.seg` exists for all `operation_record[]`, if any does not exist, send a `PreProvisionBookSegment(book_id, missing_segment)` to [[#Provisioner]] via its channel and wait for completion.
2. Iterate over the elements of `operation_record[]` collecting all elements with the same `target_segment` as the first until a different `target_segment` is encountered. 
	1. Open `book_segments/<book_id first 6 digits>/<book_state.id>-<target_segment>.seg` then write the entire group collected so far starting from the first `target_offset`
	2. loop back to `#1` until we reach the end of `operation_record[]`
3. Open `<book_state.id>.cov`. Starting from the `current_segment=checkpoint_segment` and `current_offset=checkpoint_offset` traverse the book journal. Inititalize `rollup_balance=running_balance`
	1. Load the `seg` file corresponding to `current_segment` and jump to `current_offset`
	2. Read the next `2 bytes` which represents the record length. If this is 0 or we've reached EOF, we've reached the end of the segment.
		1. Verify next segment by loading the `seg` file `current_segment+1` at `offset=0`. Read the next `2 bytes` which represents the record length, if this is 0, we've reached the end of the journal. Initialize `checkpoint_segment=current_segment`, `checkpoint_offset=current_offset`.
		2. Otherwise, set `current_segment=current_segment+1` and `current_offset=0` then loop back to `#4.2` 
	3. Read the next `x bytes` where `x` is the record length ([[#Book Record]]) then add the record's `amount` to `rollup_balance` then loop back to `#3.2` 
4. write `checkpoint_segment`, `checkpoint_offset`, and `running_balance` to `<book_state.id>.cov`
5. update `operation.cov` with the furthest `operation_segment` and `operation_offset` safely applied across all books
##### when `shutdown signal` is received
1. stop processing `Apply()` messages immediately

##### System Failure
- when the applier encounters any form of IO error
	1. stop processing `Apply()` messages immediately
	2. trigger a system shutdown
#### Provisioner
##### PreProvisionOperationSegment(segment_number)
1. pre-allocate a `FIXED_SEGMENT_LENGTH` sized file in `operation_segments/<segment_number>.seg
##### PreProvisionBookSegment(book_id, segment_number)
1. pre-allocate a `FIXED_SEGMENT_LENGTH` sized file in `book_segments/<book_id first 6 digits>/<book_id>-<segment_number>.seg
### Persistent Primitives

### Journals

The whole persistence layer of the ledger is structured around "event" sourcing journals. Journals have 3 main components:
1. A **cover** file which is the first contact point and contains metadata that's required to traverse the segments.
2. Multiple **segment** files which are pre-allocated append-only files that that divide the entire journal sequence into manageable blocks. Segments contain sequenced records.
3. An **record** is a unit of data that describes a certain action that is done on the state of the account. Applying each record record in the final state of whichever domain entity the journal represents.

Journals come in two variants:
- **Book Journals** are actual DEBIT/CREDIT records performed against the account. Traversing this gives us the current balance of that particular book.
- **Operation Journals** are more like commit logs whose records contain literal instructions on what offset of which segment of which book to write a sequence of bytes into. Each record is a collection of multiple write instructions that make up one atomic transaction whose successful write signals the system to acknowledge the request handler that initiated it. Traversing the Operation Journal gives us the current state of ALL books in the system.
### Covers
- All covers have a `checkpoint_offset` and `checkpoint_segment` which describes the coordinates for which the rollup has last stopped.
	- For Books, this describes up to what record the cover's `running_balance` was based on
	- For Operations, this describes up to what record has been applied to their respective books

### Records
- All records have a a header section which describes the `record_length` that allows readers to quickly skip over records.

### Persistence Models
### Operation Journal
cover filename: `operation.cov`
segment filename: `operation_segments/<u32 segment_number>.seg`
#### Operation Journal cover

| Field                | type  | Notes                                                                                                                                                                                                                       |     |
| -------------------- | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --- |
| `checkpoint_segment` | `u32` | Segment of the furthest position whose records have been verified as applied to their target books.                                                                                                                         |     |
| `checkpoint_offset`  | `u32` | Offset of the furthest position whose records have been verified as applied to their target books. This describes the offset of the last bit of the last verified record such that`checkpoint_offset+1` is the next record. |     |
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
cover filename: `<u32 book_id>.cov`
segment filename: `book_segments/<book_id first 6 digits>/<u32 book_id>-<u32 segment_number>.seg`

#### Book Journal cover

| Field                | type             | Notes                                                                                                                      |
| -------------------- | ---------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `balance_type`       | enum, `repr(u8)` | This book's balance type. `AVAILABLE`, `CURRENT`, `HOLD`                                                                   |
| `accounting_type`    | enum, `repr(u8)` | Natural direction of this book. Fixed at creation. Drives DR/CR-to-signed translation at pre-acceptance. `DEBIT`, `CREDIT` |
| `allow_overdraft`    | `bool`           | Whether this book may be driven negative by an accepted Operation. Fixed at creation.                                      |
| `running_balance`    | `i128`           | Signed minor units, natural-direction convention. Must be updated through a overflow-safe addition.                        |
| `checkpoint_segment` | `u32`            | Segment of the last Book Record position folded into `running_balance`.                                                    |
| `checkpoint_offset`  | `u32`            | Offset of the last Book Record position folded into `running_balance`.                                                     |

#### Book Record

| Field           | type      | Notes                                                                                  |
| --------------- | --------- | -------------------------------------------------------------------------------------- |
| `record_length` | `u8`      | Total bytes of record including the `record_length`.                                   |
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

