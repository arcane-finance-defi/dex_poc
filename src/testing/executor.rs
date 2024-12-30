use alloc::{sync::Arc, vec::Vec};
use miden_objects::{notes::NoteId, transaction::ExecutedTransaction};
use miden_tx::{testing::TransactionContext, TransactionExecutor, TransactionExecutorError};

pub fn execute_with_debugger(ctx: TransactionContext) -> Result<ExecutedTransaction, TransactionExecutorError> {
    let account_id = ctx.account().id();
    let block_num = ctx.tx_inputs().block_header().block_num();
    let notes: Vec<NoteId> =
        ctx.tx_inputs().input_notes().into_iter().map(|n| n.id()).collect();

    let tx_executor = TransactionExecutor::new(Arc::new(ctx.tx_inputs().clone()), None)
        .with_debug_mode(true);

    tx_executor.execute_transaction(account_id, block_num, &notes, ctx.tx_args().clone())
}