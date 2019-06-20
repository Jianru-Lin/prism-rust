use crate::block::{Block, Content};
use crate::blockchain::BlockChain;
use crate::blockdb::BlockDatabase;
use crate::crypto::hash::Hashable;
use crate::miner::memory_pool::MemoryPool;
use crate::network::message;
use crate::network::server::Handle as ServerHandle;
use crate::transaction::Transaction;
use crate::utxodb::UtxoDatabase;
use crate::wallet::Wallet;
use crate::experiment::performance_counter::PERFORMANCE_COUNTER;
use std::sync::Mutex;
use log::{debug, info, warn};

pub fn new_validated_block(
    block: &Block,
    mempool: &Mutex<MemoryPool>,
    blockdb: &BlockDatabase,
    chain: &BlockChain,
    server: &ServerHandle,
) {
    PERFORMANCE_COUNTER.record_process_block(&block);

    // if this block is a transaction, remove transactions from mempool
    match &block.content {
        Content::Transaction(content) => {
            let mut mempool = mempool.lock().unwrap();
            for tx in &content.transactions {
                mempool.remove_by_hash(&tx.hash());
                // the inputs have been used here, so remove all transactions in the mempool that
                // tries to use the input again.
                for input in tx.input.iter() {
                    mempool.remove_by_input(input);
                }
            }
            drop(mempool);
        }
        _ => (),
    };

    // insert the new block into the blockchain
    chain.insert_block(&block).unwrap();
    info!("Block Path 4: Added block {} to blockchain", block.hash());

}
