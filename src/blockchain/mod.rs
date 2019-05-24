use crate::block::{Block, Content};
use crate::config::*;
use crate::crypto::hash::{Hashable, H256};

use bigint::uint::U256;
use bincode::{deserialize, serialize};
use log::{debug, info, trace};
use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::mem;
use std::sync::Mutex;

// Column family names for node/chain metadata
const PROPOSER_NODE_LEVEL_CF: &str = "PROPOSER_NODE_LEVEL"; // hash to node level (u64)
const VOTER_NODE_LEVEL_CF: &str = "VOTER_NODE_LEVEL"; // hash to node level (u64)
const VOTER_NODE_CHAIN_CF: &str = "VOTER_NODE_CHAIN"; // hash to chain number (u16)
const PROPOSER_TREE_LEVEL_CF: &str = "PROPOSER_TREE_LEVEL"; // level (u64) to hashes of blocks (Vec<hash>)
const VOTER_NODE_VOTED_LEVEL_CF: &str = "VOTER_NODE_VOTED_LEVEL"; // hash to max. voted level (u64)
const PROPOSER_NODE_VOTE_CF: &str = "PROPOSER_NODE_VOTE"; // hash to level and chain number of main chain votes (Vec<u16, u64>)
const PROPOSER_LEADER_SEQUENCE_CF: &str = "PROPOSER_LEADER_SEQUENCE"; // level (u64) to hash of leader block.
const PROPOSER_LEDGER_ORDER_CF: &str = "PROPOSER_LEDGER_ORDER"; // level (u64) to the list of proposer blocks confirmed
                                                                // by this level, including the leader itself. The list
                                                                // is in the order that those blocks should live in the ledger.
const PROPOSER_BLOCK_TIMESTAMP_CF: &str = "PROPOSER_BLOCK_TIMESTAMP"; // Timestamp of proposer blocks mined on levels which are integer multiples DIFFICULTY_CHANGE_INTERVAL.
const MINING_DIFFICULTY_CF: &str = "MINING_DIFFICULTY"; // Mining difficulty if mined on proposer parents.

// Column family names for graph neighbors
const PARENT_NEIGHBOR_CF: &str = "GRAPH_PARENT_NEIGHBOR"; // the proposer parent of a block
const VOTE_NEIGHBOR_CF: &str = "GRAPH_VOTE_NEIGHBOR"; // neighbors associated by a vote
const VOTER_PARENT_NEIGHBOR_CF: &str = "GRAPH_VOTER_PARENT_NEIGHBOR"; // the voter parent of a block
const TRANSACTION_REF_NEIGHBOR_CF: &str = "GRAPH_TRANSACTION_REF_NEIGHBOR";
const PROPOSER_REF_NEIGHBOR_CF: &str = "GRAPH_PROPOSER_REF_NEIGHBOR";

pub type Result<T> = std::result::Result<T, rocksdb::Error>;

// cf_handle is a lightweight operation, it takes 44000 micro seconds to get 100000 cf handles

pub struct BlockChain {
    db: DB,
    proposer_best: Mutex<(H256, u64)>,
    voter_best: Vec<Mutex<(H256, u64)>>,
    unreferred_transaction: Mutex<HashSet<H256>>,
    unreferred_proposer: Mutex<HashSet<H256>>,
    unconfirmed_proposer: Mutex<HashSet<H256>>,
    ledger_tip: Mutex<u64>,
}

// Functions to edit the blockchain
impl BlockChain {
    /// Open the blockchain database at the given path, and create missing column families.
    /// This function also populates the metadata fields with default values, and those
    /// fields must be initialized later.
    fn open<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let proposer_node_level_cf =
            ColumnFamilyDescriptor::new(PROPOSER_NODE_LEVEL_CF, Options::default());
        let voter_node_level_cf =
            ColumnFamilyDescriptor::new(VOTER_NODE_LEVEL_CF, Options::default());
        let voter_node_chain_cf =
            ColumnFamilyDescriptor::new(VOTER_NODE_CHAIN_CF, Options::default());
        let voter_node_voted_level_cf =
            ColumnFamilyDescriptor::new(VOTER_NODE_VOTED_LEVEL_CF, Options::default());
        let proposer_leader_sequence_cf =
            ColumnFamilyDescriptor::new(PROPOSER_LEADER_SEQUENCE_CF, Options::default());
        let proposer_ledger_order_cf =
            ColumnFamilyDescriptor::new(PROPOSER_LEDGER_ORDER_CF, Options::default());
        let proposer_block_timestamp_cf =
            ColumnFamilyDescriptor::new(PROPOSER_BLOCK_TIMESTAMP_CF, Options::default());
        let mining_difficulty_cf =
            ColumnFamilyDescriptor::new(MINING_DIFFICULTY_CF, Options::default());

        let mut proposer_tree_level_option = Options::default();
        proposer_tree_level_option.set_merge_operator(
            "append H256 vec",
            h256_vec_append_merge,
            None,
        );
        let proposer_tree_level_cf =
            ColumnFamilyDescriptor::new(PROPOSER_TREE_LEVEL_CF, proposer_tree_level_option);

        let mut proposer_node_vote_option = Options::default();
        proposer_node_vote_option.set_merge_operator("insert or remove vote", vote_vec_merge, None);
        let proposer_node_vote_cf =
            ColumnFamilyDescriptor::new(PROPOSER_NODE_VOTE_CF, proposer_node_vote_option);

        let mut parent_neighbor_option = Options::default();
        parent_neighbor_option.set_merge_operator("append H256 vec", h256_vec_append_merge, None);
        let parent_neighbor_cf =
            ColumnFamilyDescriptor::new(PARENT_NEIGHBOR_CF, parent_neighbor_option);

        let mut vote_neighbor_option = Options::default();
        vote_neighbor_option.set_merge_operator("append H256 vec", h256_vec_append_merge, None);
        let vote_neighbor_cf = ColumnFamilyDescriptor::new(VOTE_NEIGHBOR_CF, vote_neighbor_option);

        let mut voter_parent_neighbor_option = Options::default();
        voter_parent_neighbor_option.set_merge_operator(
            "append H256 vec",
            h256_vec_append_merge,
            None,
        );
        let voter_parent_neighbor_cf =
            ColumnFamilyDescriptor::new(VOTER_PARENT_NEIGHBOR_CF, voter_parent_neighbor_option);

        let mut transaction_ref_neighbor_option = Options::default();
        transaction_ref_neighbor_option.set_merge_operator(
            "append H256 vec",
            h256_vec_append_merge,
            None,
        );
        let transaction_ref_neighbor_cf = ColumnFamilyDescriptor::new(
            TRANSACTION_REF_NEIGHBOR_CF,
            transaction_ref_neighbor_option,
        );

        let mut proposer_ref_neighbor_option = Options::default();
        proposer_ref_neighbor_option.set_merge_operator(
            "append H256 vec",
            h256_vec_append_merge,
            None,
        );
        let proposer_ref_neighbor_cf =
            ColumnFamilyDescriptor::new(PROPOSER_REF_NEIGHBOR_CF, proposer_ref_neighbor_option);

        let cfs = vec![
            proposer_node_level_cf,
            voter_node_level_cf,
            voter_node_chain_cf,
            voter_node_voted_level_cf,
            proposer_leader_sequence_cf,
            proposer_ledger_order_cf,
            proposer_tree_level_cf,
            proposer_node_vote_cf,
            mining_difficulty_cf,
            proposer_block_timestamp_cf,
            parent_neighbor_cf,
            vote_neighbor_cf,
            voter_parent_neighbor_cf,
            transaction_ref_neighbor_cf,
            proposer_ref_neighbor_cf,
        ];
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        let db = DB::open_cf_descriptors(&opts, path, cfs)?;
        let mut voter_best: Vec<Mutex<(H256, u64)>> = vec![];
        for _ in 0..NUM_VOTER_CHAINS {
            voter_best.push(Mutex::new((H256::default(), 0)));
        }

        let blockchain_db = Self {
            db: db,
            proposer_best: Mutex::new((H256::default(), 0)),
            voter_best: voter_best,
            unreferred_transaction: Mutex::new(HashSet::new()),
            unreferred_proposer: Mutex::new(HashSet::new()),
            unconfirmed_proposer: Mutex::new(HashSet::new()),
            ledger_tip: Mutex::new(0),
        };

        return Ok(blockchain_db);
    }

    /// Destroy the existing database at the given path, create a new one, and initialize the content.
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        DB::destroy(&Options::default(), &path)?;
        let db = Self::open(&path)?;
        // get cf handles
        let proposer_node_level_cf = db.db.cf_handle(PROPOSER_NODE_LEVEL_CF).unwrap();
        let voter_node_level_cf = db.db.cf_handle(VOTER_NODE_LEVEL_CF).unwrap();
        let voter_node_chain_cf = db.db.cf_handle(VOTER_NODE_CHAIN_CF).unwrap();
        let voter_node_voted_level_cf = db.db.cf_handle(VOTER_NODE_VOTED_LEVEL_CF).unwrap();
        let proposer_node_vote_cf = db.db.cf_handle(PROPOSER_NODE_VOTE_CF).unwrap();
        let proposer_tree_level_cf = db.db.cf_handle(PROPOSER_TREE_LEVEL_CF).unwrap();
        let parent_neighbor_cf = db.db.cf_handle(PARENT_NEIGHBOR_CF).unwrap();
        let vote_neighbor_cf = db.db.cf_handle(VOTE_NEIGHBOR_CF).unwrap();
        let proposer_leader_sequence_cf = db.db.cf_handle(PROPOSER_LEADER_SEQUENCE_CF).unwrap();
        let proposer_ledger_order_cf = db.db.cf_handle(PROPOSER_LEDGER_ORDER_CF).unwrap();
        let proposer_ref_neighbor_cf = db.db.cf_handle(PROPOSER_REF_NEIGHBOR_CF).unwrap();
        let transaction_ref_neighbor_cf = db.db.cf_handle(TRANSACTION_REF_NEIGHBOR_CF).unwrap();
        let mining_difficulty_cf = db.db.cf_handle(MINING_DIFFICULTY_CF).unwrap();

        // insert genesis blocks
        let mut wb = WriteBatch::default();

        // proposer genesis block
        wb.put_cf(
            proposer_node_level_cf,
            serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
            serialize(&(0 as u64)).unwrap(),
        )?;
        wb.merge_cf(
            proposer_tree_level_cf,
            serialize(&(0 as u64)).unwrap(),
            serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
        )?;
        wb.put_cf(
            mining_difficulty_cf,
            serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
            serialize(&(*INITIAL_DIFFICULTY)).unwrap(),
        );
        let mut proposer_best = db.proposer_best.lock().unwrap();
        proposer_best.0 = *PROPOSER_GENESIS_HASH;
        drop(proposer_best);
        let mut unreferred_proposer = db.unreferred_proposer.lock().unwrap();
        unreferred_proposer.insert(*PROPOSER_GENESIS_HASH);
        drop(unreferred_proposer);
        wb.put_cf(
            proposer_leader_sequence_cf,
            serialize(&(0 as u64)).unwrap(),
            serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
        )?;
        let proposer_genesis_ledger: Vec<H256> = vec![*PROPOSER_GENESIS_HASH];
        wb.put_cf(
            proposer_ledger_order_cf,
            serialize(&(0 as u64)).unwrap(),
            serialize(&proposer_genesis_ledger).unwrap(),
        )?;
        wb.put_cf(
            proposer_ref_neighbor_cf,
            serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
            serialize(&Vec::<H256>::new()).unwrap(),
        )?;
        wb.put_cf(
            transaction_ref_neighbor_cf,
            serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
            serialize(&Vec::<H256>::new()).unwrap(),
        )?;

        // voter genesis blocks
        for chain_num in 0..NUM_VOTER_CHAINS {
            wb.put_cf(
                parent_neighbor_cf,
                serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
            )?;
            wb.merge_cf(
                vote_neighbor_cf,
                serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
            )?;
            wb.merge_cf(
                proposer_node_vote_cf,
                serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
                serialize(&(true, chain_num as u16, 0 as u64)).unwrap(),
            )?;
            wb.put_cf(
                voter_node_level_cf,
                serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                serialize(&(0 as u64)).unwrap(),
            )?;
            wb.put_cf(
                voter_node_voted_level_cf,
                serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                serialize(&(0 as u64)).unwrap(),
            )?;
            wb.put_cf(
                voter_node_chain_cf,
                serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                serialize(&(chain_num as u16)).unwrap(),
            )?;
            let mut voter_best = db.voter_best[chain_num as usize].lock().unwrap();
            voter_best.0 = VOTER_GENESIS_HASHES[chain_num as usize];
            drop(voter_best);
        }
        db.db.write(wb)?;

        return Ok(db);
    }

    /// Insert a new block into the ledger. Returns the list of added transaction blocks and
    /// removed transaction blocks.
    pub fn insert_block(&self, block: &Block) -> Result<(Vec<H256>, Vec<H256>)> {
        // get cf handles
        let proposer_node_level_cf = self.db.cf_handle(PROPOSER_NODE_LEVEL_CF).unwrap();
        let voter_node_level_cf = self.db.cf_handle(VOTER_NODE_LEVEL_CF).unwrap();
        let voter_node_chain_cf = self.db.cf_handle(VOTER_NODE_CHAIN_CF).unwrap();
        let voter_node_voted_level_cf = self.db.cf_handle(VOTER_NODE_VOTED_LEVEL_CF).unwrap();
        let proposer_tree_level_cf = self.db.cf_handle(PROPOSER_TREE_LEVEL_CF).unwrap();
        let proposer_node_vote_cf = self.db.cf_handle(PROPOSER_NODE_VOTE_CF).unwrap();
        let parent_neighbor_cf = self.db.cf_handle(PARENT_NEIGHBOR_CF).unwrap();
        let vote_neighbor_cf = self.db.cf_handle(VOTE_NEIGHBOR_CF).unwrap();
        let voter_parent_neighbor_cf = self.db.cf_handle(VOTER_PARENT_NEIGHBOR_CF).unwrap();
        let transaction_ref_neighbor_cf = self.db.cf_handle(TRANSACTION_REF_NEIGHBOR_CF).unwrap();
        let proposer_ref_neighbor_cf = self.db.cf_handle(PROPOSER_REF_NEIGHBOR_CF).unwrap();
        let proposer_leader_sequence_cf = self.db.cf_handle(PROPOSER_LEADER_SEQUENCE_CF).unwrap();
        let proposer_ledger_order_cf = self.db.cf_handle(PROPOSER_LEDGER_ORDER_CF).unwrap();
        let proposer_block_timestamp_cf = self.db.cf_handle(PROPOSER_BLOCK_TIMESTAMP_CF).unwrap();
        let mining_difficulty_cf = self.db.cf_handle(MINING_DIFFICULTY_CF).unwrap();

        let mut wb = WriteBatch::default();

        // insert parent link
        let block_hash = block.hash();
        let parent_hash = block.header.parent;
        wb.put_cf(
            parent_neighbor_cf,
            serialize(&block_hash).unwrap(),
            serialize(&parent_hash).unwrap(),
        )?;

        match &block.content {
            Content::Proposer(content) => {
                // remove ref'ed blocks from unreferred list, mark itself as unreferred
                let mut unreferred_proposer = self.unreferred_proposer.lock().unwrap();
                for ref_hash in &content.proposer_refs {
                    unreferred_proposer.remove(&ref_hash);
                }
                unreferred_proposer.remove(&parent_hash);
                unreferred_proposer.insert(block_hash);
                let mut unreferred_transaction = self.unreferred_transaction.lock().unwrap();
                for ref_hash in &content.transaction_refs {
                    unreferred_transaction.remove(&ref_hash);
                }
                // add ref'ed blocks
                // note that the parent is the first proposer block that we refer
                let mut refed_proposer: Vec<H256> = vec![parent_hash];
                refed_proposer.extend(&content.proposer_refs);
                wb.put_cf(
                    proposer_ref_neighbor_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&refed_proposer).unwrap(),
                )?;
                wb.put_cf(
                    transaction_ref_neighbor_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&content.transaction_refs).unwrap(),
                )?;
                // get current block level
                let parent_level: u64 = deserialize(
                    &self
                        .db
                        .get_cf(proposer_node_level_cf, serialize(&parent_hash).unwrap())?
                        .unwrap(),
                )
                .unwrap();
                let self_level = parent_level + 1;
                // set current block level
                wb.put_cf(
                    proposer_node_level_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&self_level).unwrap(),
                )?;
                wb.merge_cf(
                    proposer_tree_level_cf,
                    serialize(&self_level).unwrap(),
                    serialize(&block_hash).unwrap(),
                )?;
                // set the mining difficulty
                let parent_difficulty: H256 = deserialize(
                    &self
                        .db
                        .get_cf(mining_difficulty_cf, serialize(&parent_hash).unwrap())?
                        .unwrap(),
                )
                .unwrap();
                if self_level % DIFFICULTY_CHANGE_INTERVAL != 0 {
                    wb.put_cf(
                        mining_difficulty_cf,
                        serialize(&block_hash).unwrap(),
                        serialize(&parent_difficulty).unwrap(),
                    );
                } else {
                    let mut ancestor_hash = parent_hash.clone();
                    // get (DIFFICULTY_CHANGE_INTERVAL-1)^th ancestor
                    for _ in 0..DIFFICULTY_CHANGE_INTERVAL - 2 {
                        //TODO: FIXME:
                        ancestor_hash = deserialize(
                            &self
                                .db
                                .get_cf(parent_neighbor_cf, serialize(&ancestor_hash).unwrap())?
                                .unwrap(),
                        )
                        .unwrap();
                    }
                    let ancestor_timestamp: u64 = deserialize(
                        &self
                            .db
                            .get_cf(
                                proposer_block_timestamp_cf,
                                serialize(&ancestor_hash).unwrap(),
                            )?
                            .unwrap(),
                    )
                    .unwrap();
                    let previous_epoch_difficulty: [u8; 32] = (&parent_difficulty).into();
                    let previous_epoch_difficulty_u256 = U256::from_big_endian(&previous_epoch_difficulty);

                    // calculate the
                    let previous_epoch_mining_rate = U256::from(
                        (((DIFFICULTY_CHANGE_INTERVAL - 1) as f32) * 10000000.0
                            / ((block.header.timestamp - ancestor_timestamp) as f32))
                            as u64,
                    );
                    let mut current_epoch_difficulty_u256 = (previous_epoch_difficulty_u256 / previous_epoch_mining_rate)
                        * U256::from((CHAIN_MINING_RATE * 10000000.0) as u64);

                    if current_epoch_difficulty_u256 < previous_epoch_difficulty_u256 {
                        current_epoch_difficulty_u256 =
                            std::cmp::max(previous_epoch_difficulty_u256 / U256::from(2), current_epoch_difficulty_u256)
                    } else {
                        current_epoch_difficulty_u256 =
                            std::cmp::min(previous_epoch_difficulty_u256 * U256::from(2), current_epoch_difficulty_u256)
                    }
                    let current_epoch_difficulty_raw: [u8; 32] = current_epoch_difficulty_u256.into();
                    let current_epoch_difficulty: H256 = (&current_epoch_difficulty_raw).into();
                    println!(
                        "Level {}. Old difficulty {} New difficulty {}. Prev epoch mining rate {}",
                        self_level, parent_difficulty, current_epoch_difficulty, previous_epoch_mining_rate
                    );
                    wb.put_cf(
                        mining_difficulty_cf,
                        serialize(&block_hash).unwrap(),
                        serialize(&current_epoch_difficulty).unwrap(),
                    );
                }
                // add the timestamp
                wb.put_cf(
                    proposer_block_timestamp_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&block.header.timestamp).unwrap(),
                );
                // set best block info
                let mut proposer_best = self.proposer_best.lock().unwrap();
                if self_level > proposer_best.1 {
                    proposer_best.0 = block_hash;
                    proposer_best.1 = self_level;
                }
                info!(
                    "Inserted: Proposer_block={:.8}; Level={}; n_tx_refs={}; timestamp={}; size={} KB",
                    block_hash,
                    self_level,
                    content.transaction_refs.len(),
                    block.header.timestamp,
                    (block.get_bytes() as f64)/1000.0
                );

                // mark this new proposer block as unconfirmed
                let mut unconfirmed_proposer = self.unconfirmed_proposer.lock().unwrap();
                unconfirmed_proposer.insert(block_hash);
                self.db.write(wb)?;
                drop(unreferred_proposer);
                drop(unreferred_transaction);
                drop(proposer_best);
                drop(unconfirmed_proposer);
                return Ok((vec![], vec![]));
            }
            Content::Voter(content) => {
                // add voter parent
                let voter_parent_hash = content.voter_parent;
                wb.put_cf(
                    voter_parent_neighbor_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&voter_parent_hash).unwrap(),
                )?;
                // get current block level and chain number
                let voter_parent_level: u64 = deserialize(
                    &self
                        .db
                        .get_cf(voter_node_level_cf, serialize(&voter_parent_hash).unwrap())?
                        .unwrap(),
                )
                .unwrap();
                let voter_parent_chain: u16 = deserialize(
                    &self
                        .db
                        .get_cf(voter_node_chain_cf, serialize(&voter_parent_hash).unwrap())?
                        .unwrap(),
                )
                .unwrap();
                let self_level = voter_parent_level + 1;
                let self_chain = voter_parent_chain;
                // set current block level and chain number
                wb.put_cf(
                    voter_node_level_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&self_level).unwrap(),
                )?;
                wb.put_cf(
                    voter_node_chain_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&self_chain).unwrap(),
                )?;
                // add voted blocks and set deepest voted level
                wb.put_cf(
                    vote_neighbor_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&content.votes).unwrap(),
                )?;
                // set the voted level to be until proposer parent
                let proposer_parent_level: u64 = deserialize(
                    &self
                        .db
                        .get_cf(proposer_node_level_cf, serialize(&parent_hash).unwrap())?
                        .unwrap(),
                )
                .unwrap();
                wb.put_cf(
                    voter_node_voted_level_cf,
                    serialize(&block_hash).unwrap(),
                    serialize(&proposer_parent_level).unwrap(),
                )?;

                let mut voter_best = self.voter_best[self_chain as usize].lock().unwrap();
                let previous_best = voter_best.0;
                let previous_best_level = voter_best.1;

                // update best block
                if self_level > voter_best.1 {
                    voter_best.0 = block_hash;
                    voter_best.1 = self_level;
                }
                trace!(
                    "Inserted: Voter_block={:.8}; Chain={}; Level={}; n_votes={}; timestamp={}; size={} KB",
                    block_hash,
                    self_chain,
                    self_level,
                    content.votes.len(),
                    block.header.timestamp,
                    (block.get_bytes() as f64)/1000.0
                );

                // update vote levels
                // only update if we are the main chain
                if self_level > previous_best_level {
                    let mut added: Vec<(H256, u64)> = vec![];
                    let mut removed: Vec<(H256, u64)> = vec![];
                    // if it's a side chain, and we are now better than the previous best, then
                    // we are now the new main chain.
                    if voter_parent_hash != previous_best {
                        let mut to_level = voter_parent_level;
                        let mut from_level = previous_best_level;
                        let mut to = voter_parent_hash;
                        let mut from = previous_best;
                        while to_level > from_level {
                            let votes: Vec<H256> = deserialize(
                                &self
                                    .db
                                    .get_cf(vote_neighbor_cf, serialize(&to).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            for vote in votes {
                                added.push((vote, to_level));
                            }
                            to = deserialize(
                                &self
                                    .db
                                    .get_cf(voter_parent_neighbor_cf, serialize(&to).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            to_level -= 1;
                        }

                        // trace back both from chain and to chain until they reach the same block
                        while to != from {
                            // trace back to chain
                            let votes: Vec<H256> = deserialize(
                                &self
                                    .db
                                    .get_cf(vote_neighbor_cf, serialize(&to).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            for vote in votes {
                                added.push((vote, to_level));
                            }
                            to = deserialize(
                                &self
                                    .db
                                    .get_cf(voter_parent_neighbor_cf, serialize(&to).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            to_level -= 1;

                            // trace back from chain
                            let votes: Vec<H256> = deserialize(
                                &self
                                    .db
                                    .get_cf(vote_neighbor_cf, serialize(&from).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            for vote in votes {
                                removed.push((vote, from_level));
                            }
                            from = deserialize(
                                &self
                                    .db
                                    .get_cf(voter_parent_neighbor_cf, serialize(&from).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            from_level -= 1;
                        }
                    }
                    // track which proposer levels did we touch. BTreeSet stores the elements in
                    // order, and its iterator will retain this order.
                    let mut affected: BTreeSet<u64> = BTreeSet::new();
                    for removed_vote in &removed {
                        wb.merge_cf(
                            proposer_node_vote_cf,
                            serialize(&removed_vote.0).unwrap(),
                            serialize(&(false, self_chain as u16, removed_vote.1)).unwrap(),
                        )?;
                        let proposer_level: u64 = deserialize(
                            &self
                                .db
                                .get_cf(proposer_node_level_cf, serialize(&removed_vote.0).unwrap())
                                .unwrap()
                                .unwrap(),
                        )
                        .unwrap();
                        affected.insert(proposer_level);
                    }
                    for added_vote in &added {
                        wb.merge_cf(
                            proposer_node_vote_cf,
                            serialize(&added_vote.0).unwrap(),
                            serialize(&(true, self_chain as u16, added_vote.1)).unwrap(),
                        )?;
                        let proposer_level: u64 = deserialize(
                            &self
                                .db
                                .get_cf(proposer_node_level_cf, serialize(&added_vote.0).unwrap())
                                .unwrap()
                                .unwrap(),
                        )
                        .unwrap();
                        affected.insert(proposer_level);
                    }
                    // finally add the new votes in this new block
                    for added_vote in &content.votes {
                        wb.merge_cf(
                            proposer_node_vote_cf,
                            serialize(&added_vote).unwrap(),
                            serialize(&(true, self_chain as u16, self_level as u64)).unwrap(),
                        )?;
                        let proposer_level: u64 = deserialize(
                            &self
                                .db
                                .get_cf(proposer_node_level_cf, serialize(&added_vote).unwrap())
                                .unwrap()
                                .unwrap(),
                        )
                        .unwrap();
                        affected.insert(proposer_level);
                    }

                    // FIXME: this is ugly. We have an ongoing write batch `wb` that contains
                    // updates to cf'es except leader sequence and ledger order (basically
                    // all topological information). Now we are going to commit this batch and use
                    // the data we just committed to update leader sequence and ledger order. To do
                    // this, we create a new write batch still named `wb` and return early here.
                    // It breaks the promise that all writes happens in this function are committed
                    // in one batch. We need to fix it if we want concurrent access to blockchain.
                    self.db.write(wb)?;
                    let mut wb = WriteBatch::default();

                    // track the smallest level whose leader has changed
                    let mut change_begin: Option<u64> = None;

                    // try to update the leader block of each touched proposer level
                    for level in &affected {
                        let proposer_blocks: Vec<H256> = match self
                            .db
                            .get_cf(proposer_tree_level_cf, serialize(&level).unwrap())?
                        {
                            None => continue,
                            Some(d) => deserialize(&d).unwrap(),
                        };

                        let existing_leader: Option<H256> = match self.db.get_cf(
                            proposer_leader_sequence_cf,
                            serialize(&(*level as u64)).unwrap(),
                        )? {
                            None => None,
                            Some(d) => Some(deserialize(&d).unwrap()),
                        };

                        let new_leader: Option<H256> = {
                            let mut new_leader: Option<H256> = None;
                            for p_block in &proposer_blocks {
                                let votes: Vec<(u16, u64)> = match self
                                    .db
                                    .get_cf(proposer_node_vote_cf, serialize(&p_block).unwrap())?
                                {
                                    None => vec![],
                                    Some(d) => deserialize(&d).unwrap(),
                                };

                                // check whether this p_block is the leader
                                if votes.len() as u16 > NUM_VOTER_CHAINS / 2 + 1 {
                                    new_leader = Some(*p_block);
                                }
                            }
                            new_leader
                        };

                        if new_leader != existing_leader {
                            trace!(
                                "Confirmed leader block at level {} at time {}",
                                level,
                                block.header.timestamp
                            );
                            match new_leader {
                                None => wb.delete_cf(
                                    proposer_leader_sequence_cf,
                                    serialize(&(*level as u64)).unwrap(),
                                )?,
                                Some(new_leader) => wb.put_cf(
                                    proposer_leader_sequence_cf,
                                    serialize(&(*level as u64)).unwrap(),
                                    serialize(&new_leader).unwrap(),
                                )?,
                            };
                            if change_begin.is_none() {
                                change_begin = Some(*level);
                            }
                        }
                    }

                    // FIXME: yet another db commit. This commits the new leader blocks.
                    self.db.write(wb)?;
                    let mut wb = WriteBatch::default();

                    // If change happens, recompute the ledger. First, retire all levels from the
                    // current ledger tip all the way to the first touched level, by deconfirming
                    // the blocks originally confirmed by those levels. Then, start confirming
                    // blocks from the first touched level until we reach a level without current
                    // leader block.
                    if change_begin.is_some() {
                        let change_begin = change_begin.unwrap();
                        let mut ledger_tip = self.ledger_tip.lock().unwrap();
                        let mut unconfirmed_proposer = self.unconfirmed_proposer.lock().unwrap();

                        let mut removed: Vec<H256> = vec![]; // proposer blocks removed from the ledger
                        let mut added: Vec<H256> = vec![]; // proposer blocks added to the ledger

                        // deconfirm the blocks
                        for level in change_begin..=*ledger_tip {
                            match self.db.get_cf(
                                proposer_ledger_order_cf,
                                serialize(&(level as u64)).unwrap(),
                            )? {
                                None => {}
                                Some(d) => {
                                    let original_ledger: Vec<H256> = deserialize(&d).unwrap();
                                    wb.delete_cf(
                                        proposer_ledger_order_cf,
                                        serialize(&(level as u64)).unwrap(),
                                    )?;
                                    for block in &original_ledger {
                                        unconfirmed_proposer.insert(*block);
                                        removed.push(*block);
                                    }
                                }
                            }
                        }

                        // recompute the ledger
                        for level in change_begin.. {
                            let leader: Option<H256> = match self.db.get_cf(
                                proposer_leader_sequence_cf,
                                serialize(&(level as u64)).unwrap(),
                            )? {
                                None => None,
                                Some(d) => Some(deserialize(&d).unwrap()),
                            };
                            match leader {
                                None => {
                                    // mark that the ledger lasts till the previous level
                                    *ledger_tip = level - 1;
                                    break;
                                }
                                Some(leader) => {
                                    // Get the collection of confirmed blocks by doing a DFS, so
                                    // that we preserve the reference order (i.e. the blocks that
                                    // is referred earlier appears in the front). We will then sort
                                    // the list (stable sort) according to the level, so we get a
                                    // total ordering by level and break ties by reference order.

                                    let mut ledger: Vec<(H256, u64)> = vec![];
                                    let mut stack: Vec<H256> = vec![leader];

                                    // Start DFS
                                    while let Some(top) = stack.pop() {
                                        // See whether this block is in the unconfirmed list. If
                                        // so, remove it. Otherwise, just pass this block.
                                        if !unconfirmed_proposer.remove(&top) {
                                            continue;
                                        }

                                        // Get info of this block.
                                        let level: u64 = deserialize(
                                            &self
                                                .db
                                                .get_cf(
                                                    proposer_node_level_cf,
                                                    serialize(&top).unwrap(),
                                                )?
                                                .unwrap(),
                                        )
                                        .unwrap();
                                        let refs: Vec<H256> = deserialize(
                                            &self
                                                .db
                                                .get_cf(
                                                    proposer_ref_neighbor_cf,
                                                    serialize(&top).unwrap(),
                                                )?
                                                .unwrap(),
                                        )
                                        .unwrap();

                                        // Insert into the ledger.
                                        ledger.push((top, level));

                                        // Search all referenced blocks. Note that we need to
                                        // reverse the order of the refs: we want the front
                                        // of the reference list to be searched first, so we need
                                        // to make sure it appears at the top of the stack.
                                        for ref_hash in refs.iter().rev() {
                                            stack.push(*ref_hash);
                                        }
                                    }

                                    // Stable sort the ledger according to level
                                    ledger.sort_by_cached_key(|k| k.1);

                                    // Write the new ledger
                                    let ledger: Vec<H256> = ledger.iter().map(|x| x.0).collect();
                                    wb.put_cf(
                                        proposer_ledger_order_cf,
                                        serialize(&(level as u64)).unwrap(),
                                        serialize(&ledger).unwrap(),
                                    )?;
                                    for block in &ledger {
                                        added.push(*block);
                                    }
                                }
                            }
                        }
                        // commit the new ledger
                        self.db.write(wb)?;

                        // If we did touch the ledger, gather the diff of the ledger in the form of transaction blocks
                        let mut removed_transaction: Vec<H256> = vec![];
                        let mut added_transaction: Vec<H256> = vec![];
                        for hash in removed {
                            let mut transactions: Vec<H256> = deserialize(
                                &self
                                    .db
                                    .get_cf(transaction_ref_neighbor_cf, serialize(&hash).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            removed_transaction.append(&mut transactions);
                        }
                        for hash in added {
                            let mut transactions: Vec<H256> = deserialize(
                                &self
                                    .db
                                    .get_cf(transaction_ref_neighbor_cf, serialize(&hash).unwrap())?
                                    .unwrap(),
                            )
                            .unwrap();
                            added_transaction.append(&mut transactions);
                        }

                        drop(voter_best);
                        drop(ledger_tip);
                        drop(unconfirmed_proposer);
                        return Ok((added_transaction, removed_transaction));
                    } else {
                        // If we didn't change any leader, and thus didn't touch the ledger
                        self.db.write(wb)?;
                        drop(voter_best);
                        return Ok((vec![], vec![]));
                    }
                } else {
                    // If we didn't update the votes.
                    self.db.write(wb)?;
                    drop(voter_best);
                    return Ok((vec![], vec![]));
                }
            }
            Content::Transaction(content) => {
                // mark itself as unreferred
                let mut unreferred_transaction = self.unreferred_transaction.lock().unwrap();
                unreferred_transaction.insert(block_hash);
                drop(unreferred_transaction);
                self.db.write(wb)?;
                // get proposer parent block level
                let parent_level: u64 = deserialize(
                    &self
                        .db
                        .get_cf(proposer_node_level_cf, serialize(&parent_hash).unwrap())?
                        .unwrap(),
                )
                .unwrap();
                trace!(
                    "Inserted: Tx_block={:.8};  Level={}; n_txs={}; timestamp={}; size={} KB",
                    block_hash,
                    parent_level,
                    content.transactions.len(),
                    block.header.timestamp,
                    (block.get_bytes() as f32) / 1000.0
                );
                return Ok((vec![], vec![]));
            }
        }
    }

    pub fn get_proposer_difficulty(&self, block_hash: &H256) -> Result<H256> {
        let mining_difficulty_cf = self.db.cf_handle(MINING_DIFFICULTY_CF).unwrap();
        let difficulty: H256 = deserialize(
            &self
                .db
                .get_cf(mining_difficulty_cf, serialize(block_hash).unwrap())?
                .unwrap(),
        ).unwrap();
        return Ok(difficulty);
    }

    pub fn best_proposer(&self) -> H256 {
        let proposer_best = self.proposer_best.lock().unwrap();
        let hash = proposer_best.0;
        drop(proposer_best);
        return hash;
    }

    pub fn best_voter(&self, chain_num: usize) -> H256 {
        let voter_best = self.voter_best[chain_num].lock().unwrap();
        let hash = voter_best.0;
        drop(voter_best);
        return hash;
    }

    pub fn unreferred_proposer(&self) -> Vec<H256> {
        // TODO: does ordering matter?
        // TODO: should remove the parent block when mining
        let unreferred_proposer = self.unreferred_proposer.lock().unwrap();
        let list: Vec<H256> = unreferred_proposer.iter().cloned().collect();
        drop(unreferred_proposer);
        return list;
    }

    pub fn unreferred_transaction(&self) -> Vec<H256> {
        // TODO: does ordering matter?
        let unreferred_transaction = self.unreferred_transaction.lock().unwrap();
        let list: Vec<H256> = unreferred_transaction.iter().cloned().collect();
        drop(unreferred_transaction);
        return list;
    }

    /// Get the list of unvoted proposer blocks that a voter chain should vote for, given the tip
    /// of the particular voter chain.
    pub fn unvoted_proposer(&self, tip: &H256) -> Result<Vec<H256>> {
        // get the deepest voted level
        let voter_node_voted_level_cf = self.db.cf_handle(VOTER_NODE_VOTED_LEVEL_CF).unwrap();
        let proposer_tree_level_cf = self.db.cf_handle(PROPOSER_TREE_LEVEL_CF).unwrap();
        let voted_level: u64 = deserialize(
            &self
                .db
                .get_cf(voter_node_voted_level_cf, serialize(&tip).unwrap())?
                .unwrap(),
        )
        .unwrap();
        // get the deepest proposer level
        let proposer_best = self.proposer_best.lock().unwrap();
        let proposer_best_level = proposer_best.1;
        drop(proposer_best);

        // get the first block we heard on each proposer level
        let mut list: Vec<H256> = vec![];
        for level in voted_level + 1..=proposer_best_level {
            let blocks: Vec<H256> = deserialize(
                &self
                    .db
                    .get_cf(proposer_tree_level_cf, serialize(&(level as u64)).unwrap())?
                    .unwrap(),
            )
            .unwrap();
            list.push(blocks[0]);
        }
        return Ok(list);
    }

    /// Get the level of the proposer block
    pub fn proposer_level(&self, hash: &H256) -> Result<u64> {
        let proposer_node_level_cf = self.db.cf_handle(PROPOSER_NODE_LEVEL_CF).unwrap();
        let level: u64 = deserialize(
            &self
                .db
                .get_cf(proposer_node_level_cf, serialize(&hash).unwrap())?
                .unwrap(),
        )
        .unwrap();
        return Ok(level);
    }

    /// Check whether the given proposer block exists in the database.
    pub fn contains_proposer(&self, hash: &H256) -> Result<bool> {
        let proposer_node_level_cf = self.db.cf_handle(PROPOSER_NODE_LEVEL_CF).unwrap();
        return match self
            .db
            .get_cf(proposer_node_level_cf, serialize(&hash).unwrap())?
        {
            Some(_) => Ok(true),
            None => Ok(false),
        };
    }

    /// Check whether the given voter block exists in the database.
    pub fn contains_voter(&self, hash: &H256) -> Result<bool> {
        let voter_node_level_cf = self.db.cf_handle(VOTER_NODE_LEVEL_CF).unwrap();
        return match self
            .db
            .get_cf(voter_node_level_cf, serialize(&hash).unwrap())?
        {
            Some(_) => Ok(true),
            None => Ok(false),
        };
    }
}

impl BlockChain {
    pub fn proposer_transaction_in_ledger(&self, limit: u64) -> Result<Vec<(H256, Vec<H256>)>> {
        let ledger_tip_ = self.ledger_tip.lock().unwrap();
        let ledger_tip = *ledger_tip_;
        drop(ledger_tip_);

        let ledger_bottom: u64 = match ledger_tip > limit {
            true => ledger_tip - limit,
            false => 0,
        };
        let proposer_ledger_order_cf = self.db.cf_handle(PROPOSER_LEDGER_ORDER_CF).unwrap();
        let transaction_ref_neighbor_cf = self.db.cf_handle(TRANSACTION_REF_NEIGHBOR_CF).unwrap();

        let mut proposer_in_ledger: Vec<H256> = vec![];
        let mut ledger: Vec<(H256, Vec<H256>)> = vec![];

        for level in ledger_bottom..=ledger_tip {
            match self
                .db
                .get_cf(proposer_ledger_order_cf, serialize(&level).unwrap())?
            {
                None => unreachable!("level <= ledger tip should have leader"),
                Some(d) => {
                    let mut blocks: Vec<H256> = deserialize(&d).unwrap();
                    proposer_in_ledger.append(&mut blocks);
                }
            }
        }

        for hash in &proposer_in_ledger {
            let blocks: Vec<H256> = match self.db.get_cf(transaction_ref_neighbor_cf, serialize(&hash).unwrap())? {
                None => unreachable!("proposer in ledger should have transaction ref in database (even for empty ref)"),
                Some(d) => deserialize(&d).unwrap(),
            };
            ledger.push((*hash, blocks));
        }
        Ok(ledger)
    }

    pub fn dump(&self, limit: u64) -> Result<String> {
        /// Struct to hold blockchain data to be dumped
        #[derive(Serialize)]
        struct Dump {
            edges: Vec<Edge>,
            proposer_levels: Vec<Vec<String>>,
            proposer_leaders: HashMap<u64, String>,
            voter_longest: Vec<String>,
            proposer_nodes: HashMap<String, Proposer>,
            voter_nodes: HashMap<String, Voter>,
            //pub transaction_unconfirmed: Vec<String>, //what is the definition of this?
            transaction_in_ledger: Vec<String>,
            transaction_unreferred: Vec<String>,
            proposer_in_ledger: Vec<String>,
        }

        #[derive(Serialize)]
        enum EdgeType {
            VoterToVoterParent,
            ProposerToProposerParent,
            VoterToProposerVote,
        }
        #[derive(Serialize)]
        struct Edge {
            from: String,
            to: String,
            edgetype: EdgeType,
        }

        #[derive(Serialize)]
        struct Proposer {
            level: u64,
            status: ProposerStatus,
            votes: u16,
        }

        #[derive(Serialize)]
        enum ProposerStatus {
            Leader,
            Others, //don't know what other status we need for proposer? like unconfirmed? unreferred?
        }

        #[derive(Serialize)]
        struct Voter {
            chain: u16,
            level: u64,
            status: VoterStatus,
            deepest_vote_level: u64,
        }

        #[derive(Serialize)]
        enum VoterStatus {
            OnMainChain,
            //            Orphan,
        }

        let proposer_tree_level_cf = self.db.cf_handle(PROPOSER_TREE_LEVEL_CF).unwrap();
        let proposer_leader_sequence_cf = self.db.cf_handle(PROPOSER_LEADER_SEQUENCE_CF).unwrap();
        let parent_neighbor_cf = self.db.cf_handle(PARENT_NEIGHBOR_CF).unwrap();
        let proposer_node_vote_cf = self.db.cf_handle(PROPOSER_NODE_VOTE_CF).unwrap();
        let voter_parent_neighbor_cf = self.db.cf_handle(VOTER_PARENT_NEIGHBOR_CF).unwrap();
        let voter_node_voted_level_cf = self.db.cf_handle(VOTER_NODE_VOTED_LEVEL_CF).unwrap();
        let proposer_ledger_order_cf = self.db.cf_handle(PROPOSER_LEDGER_ORDER_CF).unwrap();
        let transaction_ref_neighbor_cf = self.db.cf_handle(TRANSACTION_REF_NEIGHBOR_CF).unwrap();

        // get the ledger tip and bottom, for processing ledger
        let ledger_tip_ = self.ledger_tip.lock().unwrap();
        let ledger_tip = *ledger_tip_;
        drop(ledger_tip_);
        let ledger_bottom: u64 = match ledger_tip > limit {
            true => ledger_tip - limit,
            false => 0,
        };

        // for computing the lowest level for voter chains related to the 100 levels of proposer nodes
        let mut voter_lowest: Vec<u64> = vec![];
        // get voter best blocks and levels, this is from memory
        let mut voter_longest: Vec<(H256, u64)> = vec![];
        for (_chain_num, voter_chain) in self.voter_best.iter().enumerate() {
            let longest = voter_chain.lock().unwrap();
            voter_longest.push((longest.0, longest.1));
            voter_lowest.push(longest.1);
        }

        // memory cache for votes
        let mut vote_cache: HashMap<(u16, u64), Vec<H256>> = HashMap::new();

        let mut edges: Vec<Edge> = vec![];
        let mut proposer_tree: HashMap<u64, Vec<H256>> = HashMap::new();
        let mut proposer_nodes: HashMap<String, Proposer> = HashMap::new();
        let mut voter_nodes: HashMap<String, Voter> = HashMap::new();
        let mut proposer_leaders: HashMap<u64, String> = HashMap::new();
        let mut proposer_in_ledger: Vec<H256> = vec![];
        let mut transaction_in_ledger: Vec<String> = vec![];

        // proposer tree
        for level in ledger_bottom.. {
            match self
                .db
                .get_cf(proposer_tree_level_cf, serialize(&level).unwrap())?
            {
                Some(d) => {
                    let blocks: Vec<H256> = deserialize(&d).unwrap();
                    proposer_tree.insert(level, blocks);
                }
                None => break,
            }
        }

        // one pass of proposer. get proposer node info, cache votes.
        for (level, blocks) in proposer_tree.iter() {
            for block in blocks {
                // get parent edges
                match self
                    .db
                    .get_cf(parent_neighbor_cf, serialize(block).unwrap())?
                {
                    Some(d) => {
                        let parent: H256 = deserialize(&d).unwrap();
                        edges.push(Edge {
                            from: block.to_string(),
                            to: parent.to_string(),
                            edgetype: EdgeType::ProposerToProposerParent,
                        });
                    }
                    None => {}
                }
                // get proposer node info
                match self
                    .db
                    .get_cf(proposer_node_vote_cf, serialize(block).unwrap())?
                {
                    Some(d) => {
                        let votes: Vec<(u16, u64)> = deserialize(&d).unwrap();
                        proposer_nodes.insert(
                            block.to_string(),
                            Proposer {
                                level: *level,
                                status: ProposerStatus::Others,
                                votes: votes.len() as u16,
                            },
                        );

                        // get voter edges
                        for (chain_num, level) in &votes {
                            if let Some(lowest) = voter_lowest.get_mut(*chain_num as usize) {
                                if *lowest > *level {
                                    *lowest = *level;
                                }
                            } else {
                                unreachable!();
                            }
                            // cache the votes
                            if let Some(v) = vote_cache.get_mut(&(*chain_num, *level)) {
                                v.push(*block);
                            } else {
                                vote_cache.insert((*chain_num, *level), vec![*block]);
                            }
                        }
                    }
                    None => {
                        proposer_nodes.insert(
                            block.to_string(),
                            Proposer {
                                level: *level,
                                status: ProposerStatus::Others,
                                votes: 0, //no votes for proposer in database, so 0 vote
                            },
                        );
                    }
                }
            }
        }

        // one pass of voters. get voter info, voter parent, and vote edges. notice the votes are cached
        for (chain_num, longest) in voter_longest.iter().enumerate() {
            let mut voter_block = longest.0;
            let mut level = longest.1;
            if let Some(lowest) = voter_lowest.get(chain_num) {
                while level >= *lowest {
                    // voter info
                    let deepest_vote_level: u64 = match self
                        .db
                        .get_cf(voter_node_voted_level_cf, serialize(&voter_block).unwrap())?
                    {
                        Some(d) => deserialize(&d).unwrap(),
                        _ => unreachable!("voter block should have voted level in database"),
                    };
                    voter_nodes.insert(
                        voter_block.to_string(),
                        Voter {
                            chain: chain_num as u16,
                            level,
                            status: VoterStatus::OnMainChain,
                            deepest_vote_level,
                        },
                    );
                    // vote edges
                    if let Some(votes) = vote_cache.get(&(chain_num as u16, level)) {
                        for vote in votes {
                            edges.push(Edge {
                                from: voter_block.to_string(),
                                to: vote.to_string(),
                                edgetype: EdgeType::VoterToProposerVote,
                            });
                        }
                    }
                    // voter parent
                    match self
                        .db
                        .get_cf(voter_parent_neighbor_cf, serialize(&voter_block).unwrap())?
                    {
                        Some(d) => {
                            let parent: H256 = deserialize(&d).unwrap();
                            edges.push(Edge {
                                from: voter_block.to_string(),
                                to: parent.to_string(),
                                edgetype: EdgeType::VoterToVoterParent,
                            });
                            voter_block = parent;
                            level -= 1;
                        }
                        None => {
                            break; // if no parent, must break the while loop
                        }
                    }
                }
            }
        }

        // proposer leader
        for level in proposer_tree.keys() {
            match self
                .db
                .get_cf(proposer_leader_sequence_cf, serialize(level).unwrap())?
            {
                Some(d) => {
                    let h256: H256 = deserialize(&d).unwrap();
                    proposer_leaders.insert(*level, h256.to_string());
                    if let Some(proposer) = proposer_nodes.get_mut(&h256.to_string()) {
                        proposer.status = ProposerStatus::Leader;
                    }
                }
                None => {}
            }
        }

        // ledger
        for level in ledger_bottom..=ledger_tip {
            match self.db.get_cf(
                proposer_ledger_order_cf,
                serialize(&(level as u64)).unwrap(),
            )? {
                None => unreachable!("level <= ledger tip should have leader"),
                Some(d) => {
                    let mut blocks: Vec<H256> = deserialize(&d).unwrap();
                    proposer_in_ledger.append(&mut blocks);
                }
            }
        }

        for hash in &proposer_in_ledger {
            match self.db.get_cf(transaction_ref_neighbor_cf, serialize(&hash).unwrap())? {
                None => unreachable!("proposer in ledger should have transaction ref in database (even for empty ref)"),
                Some(d) => {
                    let blocks: Vec<H256> = deserialize(&d).unwrap();
                    let mut blocks = blocks.into_iter().map(|h|h.to_string()).collect();
                    transaction_in_ledger.append(&mut blocks);
                }
            }
        }

        let transaction_unreferred_ = self.unreferred_transaction.lock().unwrap();
        let transaction_unreferred: Vec<String> = transaction_unreferred_
            .iter()
            .map(|h| h.to_string())
            .collect();
        drop(transaction_unreferred_);

        let proposer_levels: Vec<Vec<String>> = proposer_tree
            .into_iter()
            .map(|(_k, v)| v.into_iter().map(|h256| h256.to_string()).collect())
            .collect();
        let voter_longest: Vec<String> = voter_longest
            .into_iter()
            .map(|(h, _u)| h.to_string())
            .collect();
        let proposer_in_ledger: Vec<String> = proposer_in_ledger
            .into_iter()
            .map(|h| h.to_string())
            .collect();
        // filter the edges for nodes_to_show
        let mut proposer_to_show: Vec<String> = proposer_nodes.keys().cloned().collect();
        let mut voter_to_show: Vec<String> = voter_nodes.keys().cloned().collect();
        proposer_to_show.append(&mut voter_to_show);
        let nodes_to_show: HashSet<String> = proposer_to_show.into_iter().collect();
        let edges: Vec<Edge> = edges
            .into_iter()
            .filter(|e| nodes_to_show.contains(&e.from) && nodes_to_show.contains(&e.to))
            .collect();

        let dump = Dump {
            edges,
            proposer_levels,
            proposer_leaders,
            voter_longest,
            proposer_nodes,
            voter_nodes,
            proposer_in_ledger,
            transaction_in_ledger,
            transaction_unreferred,
        };

        Ok(serde_json::to_string_pretty(&dump).unwrap())
    }
}

fn vote_vec_merge(
    _: &[u8],
    existing_val: Option<&[u8]>,
    operands: &mut rocksdb::merge_operator::MergeOperands,
) -> Option<Vec<u8>> {
    let mut existing: Vec<(u16, u64)> = match existing_val {
        Some(v) => deserialize(v).unwrap(),
        None => vec![],
    };
    for op in operands {
        // parse the operation as add(true)/remove(false), chain(u16), level(u64)
        let operation: (bool, u16, u64) = deserialize(op).unwrap();
        match operation.0 {
            true => {
                existing.push((operation.1, operation.2));
            }
            false => {
                match existing.iter().position(|&x| x.0 == operation.1) {
                    Some(p) => existing.swap_remove(p),
                    None => continue, // TODO: potential bug here - what if we delete a nonexisting item
                };
            }
        }
    }
    let result: Vec<u8> = serialize(&existing).unwrap();
    return Some(result);
}

fn h256_vec_append_merge(
    _: &[u8],
    existing_val: Option<&[u8]>,
    operands: &mut rocksdb::merge_operator::MergeOperands,
) -> Option<Vec<u8>> {
    let mut existing: Vec<H256> = match existing_val {
        Some(v) => deserialize(v).unwrap(),
        None => vec![],
    };
    for op in operands {
        let new_hash: H256 = deserialize(op).unwrap();
        existing.push(new_hash);
    }
    let result: Vec<u8> = serialize(&existing).unwrap();
    return Some(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{proposer, transaction, voter, Block, Content};
    use crate::crypto::hash::H256;

    #[test]
    fn initialize_new() {
        let db = BlockChain::new("/tmp/prism_test_blockchain_new.rocksdb").unwrap();
        // get cf handles
        let proposer_node_vote_cf = db.db.cf_handle(PROPOSER_NODE_VOTE_CF).unwrap();
        let proposer_node_level_cf = db.db.cf_handle(PROPOSER_NODE_LEVEL_CF).unwrap();
        let voter_node_level_cf = db.db.cf_handle(VOTER_NODE_LEVEL_CF).unwrap();
        let voter_node_chain_cf = db.db.cf_handle(VOTER_NODE_CHAIN_CF).unwrap();
        let voter_node_voted_level_cf = db.db.cf_handle(VOTER_NODE_VOTED_LEVEL_CF).unwrap();
        let proposer_tree_level_cf = db.db.cf_handle(PROPOSER_TREE_LEVEL_CF).unwrap();
        let parent_neighbor_cf = db.db.cf_handle(PARENT_NEIGHBOR_CF).unwrap();
        let vote_neighbor_cf = db.db.cf_handle(VOTE_NEIGHBOR_CF).unwrap();
        let proposer_leader_sequence_cf = db.db.cf_handle(PROPOSER_LEADER_SEQUENCE_CF).unwrap();
        let proposer_ledger_order_cf = db.db.cf_handle(PROPOSER_LEDGER_ORDER_CF).unwrap();

        // validate proposer genesis
        let genesis_level: u64 = deserialize(
            &db.db
                .get_cf(
                    proposer_node_level_cf,
                    serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(genesis_level, 0);
        let level_0_blocks: Vec<H256> = deserialize(
            &db.db
                .get_cf(proposer_tree_level_cf, serialize(&(0 as u64)).unwrap())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(level_0_blocks, vec![*PROPOSER_GENESIS_HASH]);
        let genesis_votes: Vec<(u16, u64)> = deserialize(
            &db.db
                .get_cf(
                    proposer_node_vote_cf,
                    serialize(&(*PROPOSER_GENESIS_HASH)).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let mut true_genesis_votes: Vec<(u16, u64)> = vec![];
        for chain_num in 0..NUM_VOTER_CHAINS {
            true_genesis_votes.push((chain_num as u16, 0));
        }
        assert_eq!(genesis_votes, true_genesis_votes);
        assert_eq!(
            *db.proposer_best.lock().unwrap(),
            (*PROPOSER_GENESIS_HASH, 0)
        );
        assert_eq!(*db.unconfirmed_proposer.lock().unwrap(), HashSet::new());
        assert_eq!(db.unreferred_proposer.lock().unwrap().len(), 1);
        assert_eq!(
            db.unreferred_proposer
                .lock()
                .unwrap()
                .contains(&(PROPOSER_GENESIS_HASH)),
            true
        );
        let level_0_leader: H256 = deserialize(
            &db.db
                .get_cf(proposer_leader_sequence_cf, serialize(&(0 as u64)).unwrap())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(level_0_leader, *PROPOSER_GENESIS_HASH);
        let level_0_confirms: Vec<H256> = deserialize(
            &db.db
                .get_cf(proposer_ledger_order_cf, serialize(&(0 as u64)).unwrap())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(level_0_confirms, vec![*PROPOSER_GENESIS_HASH]);

        // validate voter genesis
        for chain_num in 0..NUM_VOTER_CHAINS {
            let genesis_level: u64 = deserialize(
                &db.db
                    .get_cf(
                        voter_node_level_cf,
                        serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                    )
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(genesis_level, 0);
            let voted_level: u64 = deserialize(
                &db.db
                    .get_cf(
                        voter_node_voted_level_cf,
                        serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                    )
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(voted_level, 0);
            let genesis_chain: u16 = deserialize(
                &db.db
                    .get_cf(
                        voter_node_chain_cf,
                        serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                    )
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(genesis_chain, chain_num as u16);
            let parent: H256 = deserialize(
                &db.db
                    .get_cf(
                        parent_neighbor_cf,
                        serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                    )
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(parent, *PROPOSER_GENESIS_HASH);
            let voted_proposer: Vec<H256> = deserialize(
                &db.db
                    .get_cf(
                        vote_neighbor_cf,
                        serialize(&VOTER_GENESIS_HASHES[chain_num as usize]).unwrap(),
                    )
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(voted_proposer, vec![*PROPOSER_GENESIS_HASH]);
            assert_eq!(
                *db.voter_best[chain_num as usize].lock().unwrap(),
                (VOTER_GENESIS_HASHES[chain_num as usize], 0)
            );
        }
    }

    #[test]
    fn insert_block() {
        let db = BlockChain::new("/tmp/prism_test_blockchain_insert_block.rocksdb").unwrap();
        // get cf handles
        let proposer_node_vote_cf = db.db.cf_handle(PROPOSER_NODE_VOTE_CF).unwrap();
        let proposer_node_level_cf = db.db.cf_handle(PROPOSER_NODE_LEVEL_CF).unwrap();
        let voter_node_level_cf = db.db.cf_handle(VOTER_NODE_LEVEL_CF).unwrap();
        let voter_node_chain_cf = db.db.cf_handle(VOTER_NODE_CHAIN_CF).unwrap();
        let voter_node_voted_level_cf = db.db.cf_handle(VOTER_NODE_VOTED_LEVEL_CF).unwrap();
        let proposer_tree_level_cf = db.db.cf_handle(PROPOSER_TREE_LEVEL_CF).unwrap();
        let parent_neighbor_cf = db.db.cf_handle(PARENT_NEIGHBOR_CF).unwrap();
        let vote_neighbor_cf = db.db.cf_handle(VOTE_NEIGHBOR_CF).unwrap();
        let voter_parent_neighbor_cf = db.db.cf_handle(VOTER_PARENT_NEIGHBOR_CF).unwrap();
        let transaction_ref_neighbor_cf = db.db.cf_handle(TRANSACTION_REF_NEIGHBOR_CF).unwrap();
        let proposer_ref_neighbor_cf = db.db.cf_handle(PROPOSER_REF_NEIGHBOR_CF).unwrap();
        let proposer_leader_sequence_cf = db.db.cf_handle(PROPOSER_LEADER_SEQUENCE_CF).unwrap();
        let proposer_ledger_order_cf = db.db.cf_handle(PROPOSER_LEDGER_ORDER_CF).unwrap();

        // Create a transaction block on the proposer genesis.
        let new_transaction_content = Content::Transaction(transaction::Content::new(vec![]));
        let new_transaction_block = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_transaction_content,
            [0; 32],
            H256::default(),
        );
        db.insert_block(&new_transaction_block).unwrap();

        let parent: H256 = deserialize(
            &db.db
                .get_cf(
                    parent_neighbor_cf,
                    serialize(&new_transaction_block.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(parent, *PROPOSER_GENESIS_HASH);
        assert_eq!(db.unreferred_transaction.lock().unwrap().len(), 1);
        assert_eq!(
            db.unreferred_transaction
                .lock()
                .unwrap()
                .contains(&new_transaction_block.hash()),
            true
        );

        // Create two proposer blocks, both attached to the genesis proposer block. The first one
        // refers to nothing (except the parent), and the second one refers to the first one and
        // the transaction block
        let new_proposer_content = Content::Proposer(proposer::Content::new(vec![], vec![]));
        let new_proposer_block_1 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [1; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_1).unwrap();

        let new_proposer_content = Content::Proposer(proposer::Content::new(
            vec![new_transaction_block.hash()],
            vec![new_proposer_block_1.hash()],
        ));
        let new_proposer_block_2 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [2; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_2).unwrap();

        let parent: H256 = deserialize(
            &db.db
                .get_cf(
                    parent_neighbor_cf,
                    serialize(&new_proposer_block_2.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(parent, *PROPOSER_GENESIS_HASH);
        let level: u64 = deserialize(
            &db.db
                .get_cf(
                    proposer_node_level_cf,
                    serialize(&new_proposer_block_2.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(level, 1);
        let level_1_blocks: Vec<H256> = deserialize(
            &db.db
                .get_cf(proposer_tree_level_cf, serialize(&(1 as u64)).unwrap())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            level_1_blocks,
            vec![new_proposer_block_1.hash(), new_proposer_block_2.hash()]
        );
        let proposer_ref: Vec<H256> = deserialize(
            &db.db
                .get_cf(
                    proposer_ref_neighbor_cf,
                    serialize(&new_proposer_block_2.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            proposer_ref,
            vec![*PROPOSER_GENESIS_HASH, new_proposer_block_1.hash()]
        );
        let transaction_ref: Vec<H256> = deserialize(
            &db.db
                .get_cf(
                    transaction_ref_neighbor_cf,
                    serialize(&new_proposer_block_2.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(transaction_ref, vec![new_transaction_block.hash()]);
        assert_eq!(
            *db.proposer_best.lock().unwrap(),
            (new_proposer_block_1.hash(), 1)
        );
        assert_eq!(db.unreferred_proposer.lock().unwrap().len(), 1);
        assert_eq!(
            db.unreferred_proposer
                .lock()
                .unwrap()
                .contains(&new_proposer_block_2.hash()),
            true
        );
        assert_eq!(db.unconfirmed_proposer.lock().unwrap().len(), 2);
        assert_eq!(
            db.unconfirmed_proposer
                .lock()
                .unwrap()
                .contains(&new_proposer_block_1.hash()),
            true
        );
        assert_eq!(
            db.unconfirmed_proposer
                .lock()
                .unwrap()
                .contains(&new_proposer_block_2.hash()),
            true
        );
        assert_eq!(db.unreferred_transaction.lock().unwrap().len(), 0);

        // Create a voter block attached to proposer block 2 and the first voter chain, and vote for proposer block 1.
        let new_voter_content = Content::Voter(voter::Content::new(
            0,
            VOTER_GENESIS_HASHES[0],
            vec![new_proposer_block_1.hash()],
        ));
        let new_voter_block = Block::new(
            new_proposer_block_2.hash(),
            0,
            0,
            H256::default(),
            vec![],
            new_voter_content,
            [3; 32],
            H256::default(),
        );
        db.insert_block(&new_voter_block).unwrap();

        let parent: H256 = deserialize(
            &db.db
                .get_cf(
                    parent_neighbor_cf,
                    serialize(&new_voter_block.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(parent, new_proposer_block_2.hash());
        let voter_parent: H256 = deserialize(
            &db.db
                .get_cf(
                    voter_parent_neighbor_cf,
                    serialize(&new_voter_block.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(voter_parent, VOTER_GENESIS_HASHES[0]);
        let level: u64 = deserialize(
            &db.db
                .get_cf(
                    voter_node_level_cf,
                    serialize(&new_voter_block.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(level, 1);
        let chain: u16 = deserialize(
            &db.db
                .get_cf(
                    voter_node_chain_cf,
                    serialize(&new_voter_block.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(chain, 0);
        let voted_level: u64 = deserialize(
            &db.db
                .get_cf(
                    voter_node_voted_level_cf,
                    serialize(&new_voter_block.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(voted_level, 1);
        let voted: Vec<H256> = deserialize(
            &db.db
                .get_cf(
                    vote_neighbor_cf,
                    serialize(&new_voter_block.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(voted, vec![new_proposer_block_1.hash()]);
        assert_eq!(
            *db.voter_best[0].lock().unwrap(),
            (new_voter_block.hash(), 1)
        );
        let votes: Vec<(u16, u64)> = deserialize(
            &db.db
                .get_cf(
                    proposer_node_vote_cf,
                    serialize(&new_proposer_block_1.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(votes, vec![(0, 1)]);

        // Create a fork of the voter chain and vote for proposer block 2.
        let new_voter_content = Content::Voter(voter::Content::new(
            0,
            VOTER_GENESIS_HASHES[0],
            vec![new_proposer_block_2.hash()],
        ));
        let new_voter_block = Block::new(
            new_proposer_block_2.hash(),
            0,
            0,
            H256::default(),
            vec![],
            new_voter_content,
            [4; 32],
            H256::default(),
        );
        db.insert_block(&new_voter_block).unwrap();
        let votes: Vec<(u16, u64)> = deserialize(
            &db.db
                .get_cf(
                    proposer_node_vote_cf,
                    serialize(&new_proposer_block_1.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(votes, vec![(0, 1)]);

        // Add to this fork, so that it becomes the longest chain.
        let new_voter_content =
            Content::Voter(voter::Content::new(0, new_voter_block.hash(), vec![]));
        let new_voter_block = Block::new(
            new_proposer_block_2.hash(),
            0,
            0,
            H256::default(),
            vec![],
            new_voter_content,
            [5; 32],
            H256::default(),
        );
        db.insert_block(&new_voter_block).unwrap();

        let votes: Vec<(u16, u64)> = deserialize(
            &db.db
                .get_cf(
                    proposer_node_vote_cf,
                    serialize(&new_proposer_block_1.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(votes, vec![]);
        let votes: Vec<(u16, u64)> = deserialize(
            &db.db
                .get_cf(
                    proposer_node_vote_cf,
                    serialize(&new_proposer_block_2.hash()).unwrap(),
                )
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(votes, vec![(0, 1)]);

        // Create a voter block on all remaining voter chains to vote for proposer block 2
        for chain_num in 1..NUM_VOTER_CHAINS {
            let new_voter_content = Content::Voter(voter::Content::new(
                0,
                VOTER_GENESIS_HASHES[chain_num as usize],
                vec![new_proposer_block_2.hash()],
            ));
            let mut random_payload: [u8; 32] = [0; 32];
            random_payload[0] = (chain_num & 0xff) as u8;
            random_payload[1] = ((chain_num >> 8) & 0xff) as u8;
            let new_voter_block = Block::new(
                new_proposer_block_2.hash(),
                0,
                0,
                H256::default(),
                vec![],
                new_voter_content,
                random_payload,
                H256::default(),
            );
            let diff = db.insert_block(&new_voter_block).unwrap();

            // Check that after we inserted more than NUM_VOTER_CHAINS/2+1 blocks, proposer block 2
            // becomes the leader
            let level_1_leader: Option<H256> = match db
                .db
                .get_cf(proposer_leader_sequence_cf, serialize(&(1 as u64)).unwrap())
                .unwrap()
            {
                Some(d) => Some(deserialize(&d).unwrap()),
                None => None,
            };
            let level_1_ledger: Option<Vec<H256>> = match db
                .db
                .get_cf(proposer_ledger_order_cf, serialize(&(1 as u64)).unwrap())
                .unwrap()
            {
                Some(d) => Some(deserialize(&d).unwrap()),
                None => None,
            };
            let num_voted = chain_num + 1;
            if num_voted as u16 == NUM_VOTER_CHAINS / 2 + 1 + 1 {
                assert_eq!(diff, (vec![new_transaction_block.hash()], vec![]));
            }
            if num_voted as u16 > NUM_VOTER_CHAINS / 2 + 1 {
                assert_eq!(level_1_leader, Some(new_proposer_block_2.hash()));
                assert_eq!(
                    level_1_ledger,
                    Some(vec![
                        new_proposer_block_2.hash(),
                        new_proposer_block_1.hash()
                    ])
                );
                assert_eq!(db.unconfirmed_proposer.lock().unwrap().len(), 0);
            } else {
                assert_eq!(level_1_leader, None);
                assert_eq!(level_1_ledger, None);
                assert_eq!(db.unconfirmed_proposer.lock().unwrap().len(), 2);
            }
        }

        // Fork all voter chains (except chain 0) to revert ledger level 1
        for chain_num in 1..NUM_VOTER_CHAINS {
            let new_voter_content = Content::Voter(voter::Content::new(
                0,
                VOTER_GENESIS_HASHES[chain_num as usize],
                vec![],
            ));
            let mut random_payload: [u8; 32] = [0; 32];
            random_payload[0] = (chain_num & 0xff) as u8;
            random_payload[1] = ((chain_num >> 8) & 0xff) as u8;
            random_payload[2] = 1;
            let new_voter_block = Block::new(
                *PROPOSER_GENESIS_HASH,
                0,
                0,
                H256::default(),
                vec![],
                new_voter_content,
                random_payload,
                H256::default(),
            );
            db.insert_block(&new_voter_block).unwrap();
            let new_voter_content =
                Content::Voter(voter::Content::new(0, new_voter_block.hash(), vec![]));
            let mut random_payload: [u8; 32] = [0; 32];
            random_payload[0] = (chain_num & 0xff) as u8;
            random_payload[1] = ((chain_num >> 8) & 0xff) as u8;
            random_payload[2] = 2;
            let new_voter_block = Block::new(
                *PROPOSER_GENESIS_HASH,
                0,
                0,
                H256::default(),
                vec![],
                new_voter_content,
                random_payload,
                H256::default(),
            );
            let diff = db.insert_block(&new_voter_block).unwrap();

            // Check that after we revert enough chains, proposer block 2 is no longer the leader
            let level_1_leader: Option<H256> = match db
                .db
                .get_cf(proposer_leader_sequence_cf, serialize(&(1 as u64)).unwrap())
                .unwrap()
            {
                Some(d) => Some(deserialize(&d).unwrap()),
                None => None,
            };
            let num_voted = NUM_VOTER_CHAINS - chain_num as u16;
            if num_voted as u16 == NUM_VOTER_CHAINS / 2 + 1 {
                assert_eq!(diff, (vec![], vec![new_transaction_block.hash()]));
            }
            if num_voted as u16 > NUM_VOTER_CHAINS / 2 + 1 {
                assert_eq!(level_1_leader, Some(new_proposer_block_2.hash()));
                assert_eq!(db.unconfirmed_proposer.lock().unwrap().len(), 0);
            } else {
                assert_eq!(level_1_leader, None);
                assert_eq!(db.unconfirmed_proposer.lock().unwrap().len(), 2);
            }
        }
    }

    #[test]
    fn best_proposer_and_voter() {
        let db =
            BlockChain::new("/tmp/prism_test_blockchain_best_proposer_and_voter.rocksdb").unwrap();
        assert_eq!(db.best_proposer(), *PROPOSER_GENESIS_HASH);
        assert_eq!(db.best_voter(0), VOTER_GENESIS_HASHES[0]);

        let new_proposer_content = Content::Proposer(proposer::Content::new(vec![], vec![]));
        let new_proposer_block = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [0; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block).unwrap();
        let new_voter_content = Content::Voter(voter::Content::new(
            0,
            VOTER_GENESIS_HASHES[0],
            vec![new_proposer_block.hash()],
        ));
        let new_voter_block = Block::new(
            new_proposer_block.hash(),
            0,
            0,
            H256::default(),
            vec![],
            new_voter_content,
            [1; 32],
            H256::default(),
        );
        db.insert_block(&new_voter_block).unwrap();
        assert_eq!(db.best_proposer(), new_proposer_block.hash());
        assert_eq!(db.best_voter(0), new_voter_block.hash());
    }

    #[test]
    fn unreferred_transaction_and_proposer() {
        let db =
            BlockChain::new("/tmp/prism_test_blockchain_unreferred_transaction_proposer.rocksdb")
                .unwrap();

        let new_transaction_content = Content::Transaction(transaction::Content::new(vec![]));
        let new_transaction_block = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_transaction_content,
            [0; 32],
            H256::default(),
        );
        db.insert_block(&new_transaction_block).unwrap();
        assert_eq!(
            db.unreferred_transaction(),
            vec![new_transaction_block.hash()]
        );
        assert_eq!(db.unreferred_proposer(), vec![*PROPOSER_GENESIS_HASH]);

        let new_proposer_content = Content::Proposer(proposer::Content::new(vec![], vec![]));
        let new_proposer_block_1 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [1; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_1).unwrap();
        assert_eq!(
            db.unreferred_transaction(),
            vec![new_transaction_block.hash()]
        );
        assert_eq!(db.unreferred_proposer(), vec![new_proposer_block_1.hash()]);

        let new_proposer_content = Content::Proposer(proposer::Content::new(
            vec![new_transaction_block.hash()],
            vec![new_proposer_block_1.hash()],
        ));
        let new_proposer_block_2 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [2; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_2).unwrap();
        assert_eq!(db.unreferred_transaction(), vec![]);
        assert_eq!(db.unreferred_proposer(), vec![new_proposer_block_2.hash()]);
    }

    #[test]
    fn unvoted_proposer() {
        let db = BlockChain::new("/tmp/prism_test_blockchain_unvoted_proposer.rocksdb").unwrap();
        assert_eq!(
            db.unvoted_proposer(&VOTER_GENESIS_HASHES[0]).unwrap(),
            vec![]
        );

        let new_proposer_content = Content::Proposer(proposer::Content::new(vec![], vec![]));
        let new_proposer_block_1 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [0; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_1).unwrap();

        let new_proposer_content = Content::Proposer(proposer::Content::new(vec![], vec![]));
        let new_proposer_block_2 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [1; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_2).unwrap();
        assert_eq!(
            db.unvoted_proposer(&VOTER_GENESIS_HASHES[0]).unwrap(),
            vec![new_proposer_block_1.hash()]
        );

        let new_voter_content = Content::Voter(voter::Content::new(
            0,
            VOTER_GENESIS_HASHES[0],
            vec![new_proposer_block_1.hash()],
        ));
        let new_voter_block = Block::new(
            new_proposer_block_2.hash(),
            0,
            0,
            H256::default(),
            vec![],
            new_voter_content,
            [2; 32],
            H256::default(),
        );
        db.insert_block(&new_voter_block).unwrap();

        assert_eq!(
            db.unvoted_proposer(&VOTER_GENESIS_HASHES[0]).unwrap(),
            vec![new_proposer_block_1.hash()]
        );
        assert_eq!(
            db.unvoted_proposer(&new_voter_block.hash()).unwrap(),
            vec![]
        );
    }

    #[test]
    fn merge_operator_u256_vec() {
        let db = BlockChain::new("/tmp/prism_test_blockchain_merge_op_u256_vec.rocksdb").unwrap();
        let cf = db.db.cf_handle(PARENT_NEIGHBOR_CF).unwrap();

        // merge with an nonexistent entry
        db.db
            .merge_cf(cf, b"testkey", serialize(&H256::default()).unwrap())
            .unwrap();
        let result: Vec<H256> =
            deserialize(&db.db.get_cf(cf, b"testkey").unwrap().unwrap()).unwrap();
        assert_eq!(result, vec![H256::default()]);

        // merge with an existing entry
        db.db
            .merge_cf(cf, b"testkey", serialize(&H256::default()).unwrap())
            .unwrap();
        db.db
            .merge_cf(cf, b"testkey", serialize(&H256::default()).unwrap())
            .unwrap();
        let result: Vec<H256> =
            deserialize(&db.db.get_cf(cf, b"testkey").unwrap().unwrap()).unwrap();
        assert_eq!(
            result,
            vec![H256::default(), H256::default(), H256::default()]
        );
    }

    #[test]
    fn merge_operator_btreemap() {
        let db = BlockChain::new("/tmp/prism_test_blockchain_merge_op_u64_vec.rocksdb").unwrap();
        let cf = db.db.cf_handle(PROPOSER_NODE_VOTE_CF).unwrap();

        // merge with an nonexistent entry
        db.db
            .merge_cf(
                cf,
                b"testkey",
                serialize(&(true, 0 as u16, 0 as u64)).unwrap(),
            )
            .unwrap();
        let result: Vec<(u16, u64)> =
            deserialize(&db.db.get_cf(cf, b"testkey").unwrap().unwrap()).unwrap();
        assert_eq!(result, vec![(0, 0)]);

        // insert
        db.db
            .merge_cf(
                cf,
                b"testkey",
                serialize(&(true, 10 as u16, 0 as u64)).unwrap(),
            )
            .unwrap();
        db.db
            .merge_cf(
                cf,
                b"testkey",
                serialize(&(true, 5 as u16, 0 as u64)).unwrap(),
            )
            .unwrap();
        let result: Vec<(u16, u64)> =
            deserialize(&db.db.get_cf(cf, b"testkey").unwrap().unwrap()).unwrap();
        assert_eq!(result, vec![(0, 0), (10, 0), (5, 0)]);

        // remove
        db.db
            .merge_cf(
                cf,
                b"testkey",
                serialize(&(false, 5 as u16, 0 as u64)).unwrap(),
            )
            .unwrap();
        let result: Vec<(u16, u64)> =
            deserialize(&db.db.get_cf(cf, b"testkey").unwrap().unwrap()).unwrap();
        assert_eq!(result, vec![(0, 0), (10, 0)]);
    }

    #[test]
    fn dump() {
        let db = BlockChain::new("/tmp/prism_test_blockchain_dump.rocksdb").unwrap();

        let new_proposer_content = Content::Proposer(proposer::Content::new(vec![], vec![]));
        let new_proposer_block_1 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [0; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_1).unwrap();

        let new_proposer_content = Content::Proposer(proposer::Content::new(vec![], vec![]));
        let new_proposer_block_2 = Block::new(
            *PROPOSER_GENESIS_HASH,
            0,
            0,
            H256::default(),
            vec![],
            new_proposer_content,
            [1; 32],
            H256::default(),
        );
        db.insert_block(&new_proposer_block_2).unwrap();

        let new_voter_content = Content::Voter(voter::Content::new(
            0,
            VOTER_GENESIS_HASHES[0],
            vec![new_proposer_block_1.hash()],
        ));
        let new_voter_block = Block::new(
            new_proposer_block_2.hash(),
            0,
            0,
            H256::default(),
            vec![],
            new_voter_content,
            [2; 32],
            H256::default(),
        );
        db.insert_block(&new_voter_block).unwrap();
        println!("{}", db.dump(100).unwrap());
    }
}
