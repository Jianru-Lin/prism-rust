#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate hex_literal;
#[macro_use]
extern crate lazy_static;

pub mod api;
pub mod block;
pub mod blockchain;
pub mod blockdb;
pub mod config;
pub mod crypto;
pub mod experiment;
pub mod handler;
pub mod miner;
pub mod network;
pub mod transaction;
pub mod utxodb;
pub mod validation;
pub mod visualization;
pub mod wallet;

use crate::utxodb::UtxoDatabase;
use bincode::serialize;
use blockchain::BlockChain;
use blockdb::BlockDatabase;
use crypto::hash::Hashable;
use crypto::sign::PubKey;
use miner::memory_pool::MemoryPool;
use std::sync::{mpsc, Arc, Mutex};
use transaction::{CoinId, Input, Output, Transaction};
use wallet::Wallet;

/// Gives 100 coins of 100 worth to every public key.
pub fn ico(
    pub_keys: Vec<PubKey>, // public keys of all the ico recipients
    utxodb: &Arc<UtxoDatabase>,
    wallet: &Arc<Wallet>,
) -> Result<(), rocksdb::Error> {
    let funding = Transaction {
        input: vec![],
        output: pub_keys
            .iter()
            .map(|pub_key| {
                (0..100).map(move |_| Output {
                    value: 100,
                    recipient: pub_key.hash().clone(),
                })
            })
            .flatten()
            .collect(),
        authorization: vec![],
    };
    let mut funding_coins: Vec<Input> = vec![];
    let transaction_hash = funding.hash();
    for (idx, output) in funding.output.iter().enumerate() {
        let id = CoinId {
            hash: transaction_hash,
            index: idx as u32,
        };
        utxodb
            .db
            .put(serialize(&id).unwrap(), serialize(&output).unwrap())?;
        let coin = Input {
            coin: id,
            value: output.value,
            owner: output.recipient,
        };
        funding_coins.push(coin);
    }
    wallet.apply_diff(&funding_coins, &vec![]).unwrap();
    Ok(())
}
