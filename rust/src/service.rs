use tonic::{Request, Response, Status};

use crate::proto::{
    BookBalance, CreateAccountRequest, CreateAccountResponse, FinancialOperationRequest,
    FinancialOperationResponse, ledger_service_server::LedgerService,
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
