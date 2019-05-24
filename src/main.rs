#[macro_use]
extern crate clap;

use log::{debug, error, info};
use prism::api::Server as ApiServer;
use prism::blockchain::BlockChain;
use prism::blockdb::BlockDatabase;
use prism::experiment::transaction_generator::TransactionGenerator;
use prism::miner;
use prism::miner::memory_pool::MemoryPool;
use prism::network::server;
use prism::network::worker;
use prism::utxodb::UtxoDatabase;
use prism::visualization::Server as VisualizationServer;
use prism::wallet::Wallet;
use std::net;
use std::process;
use std::sync::mpsc;
use std::sync::Arc;

fn main() {
    // parse command line arguments
    let matches = clap_app!(Prism =>
     (version: "0.1")
     (about: "Prism blockchain full client")
     (@arg verbose: -v ... "Increases the verbosity of logging")
     (@arg peer_addr: --p2p [ADDR] default_value("127.0.0.1:6000") "Sets the IP address and the port of the P2P server")
     (@arg api_addr: --api [ADDR] default_value("127.0.0.1:7000") "Sets the IP address and the port of the API server")
     (@arg visualization: --visual [ADDR] "Enables the visualization server at the given address and port")
     (@arg mine: -m --mine "Starts the CPU miner as the client starts")
     (@arg known_peer: -c --connect ... [PEER] "Sets the peers to connect to")
     (@arg block_db: --blockdb [PATH] default_value("/tmp/prism-blocks.rocksdb") "Sets the path of the block database")
     (@arg utxo_db: --utxodb [PATH] default_value("/tmp/prism-utxo.rocksdb") "Sets the path of the UTXO database")
     (@arg blockchain_db: --blockchaindb [PATH] default_value("/tmp/prism-blockchain.rocksdb") "Sets the path of the blockchain database")
     (@arg wallet_db: --walletdb [PATH] default_value("/tmp/prism-wallet.rocksdb") "Sets the path of the wallet")
    )
    .get_matches();

    // init logger
    let verbosity = matches.occurrences_of("verbose") as usize;
    stderrlog::new().verbosity(verbosity).init().unwrap();

    // init mempool
    let mempool = MemoryPool::new();
    let mempool = Arc::new(std::sync::Mutex::new(mempool));

    // init block database
    let blockdb = BlockDatabase::new(&matches.value_of("block_db").unwrap()).unwrap();
    let blockdb = Arc::new(blockdb);

    // init utxo database
    let utxodb = UtxoDatabase::new(&matches.value_of("utxo_db").unwrap()).unwrap();
    let utxodb = Arc::new(utxodb);

    // init blockchain database
    let blockchain = BlockChain::new(&matches.value_of("blockchain_db").unwrap()).unwrap();
    let blockchain = Arc::new(blockchain);

    // init wallet database
    let wallet = Wallet::new(&matches.value_of("wallet_db").unwrap()).unwrap();
    let wallet = Arc::new(wallet);

    // parse p2p server address
    let p2p_addr = matches
        .value_of("peer_addr")
        .unwrap()
        .parse::<net::SocketAddr>()
        .unwrap_or_else(|e| {
            error!("Error parsing P2P server address: {}", e);
            process::exit(1);
        });

    // parse api server address
    let api_addr = matches
        .value_of("api_addr")
        .unwrap()
        .parse::<net::SocketAddr>()
        .unwrap_or_else(|e| {
            error!("Error parsing API server address: {}", e);
            process::exit(1);
        });

    // create channels between server and worker, worker and miner, miner and worker
    let (msg_tx, msg_rx) = mpsc::channel();
    let (ctx_tx, ctx_rx) = mpsc::channel();

    // start the p2p server
    let (server_ctx, server) = server::new(p2p_addr, msg_tx).unwrap();
    server_ctx.start().unwrap();

    // start the worker
    let worker_ctx = worker::new(
        4,
        msg_rx,
        &blockchain,
        &blockdb,
        &utxodb,
        &wallet,
        &mempool,
        ctx_tx,
        &server,
    );
    worker_ctx.start();

    // start the miner
    let (miner_ctx, miner) = miner::new(
        &mempool,
        &blockchain,
        &utxodb,
        &wallet,
        &blockdb,
        ctx_rx,
        &server,
    );
    miner_ctx.start();

    // connect to known peers
    if let Some(known_peers) = matches.values_of("known_peer") {
        for peer in known_peers {
            let addr = match peer.parse::<net::SocketAddr>() {
                Ok(x) => x,
                Err(e) => {
                    error!("Error parsing peer address {}: {}", &peer, e);
                    continue;
                }
            };
            match server.connect(addr) {
                Ok(_) => info!("Connected to outgoing peer {}", &addr),
                Err(e) => error!("Error connecting to peer {}: {}", addr, e),
            }
        }
    }

    // TODO: make it a seaprate API
    wallet.generate_keypair().unwrap();

    // start the transaction generator
    let (txgen_ctx, txgen_control_chan) = TransactionGenerator::new(&wallet, &server, &mempool);
    txgen_ctx.start();

    // start the API server
    ApiServer::start(api_addr, &wallet, &server, &mempool, txgen_control_chan);

    // start the miner into running mode
    // TODO: make it a separate API
    if matches.is_present("mine") {
        miner.start();
    }

    // start the visualization server
    match matches.value_of("visualization") {
        Some(addr) => {
            let addr = addr.parse::<net::SocketAddr>().unwrap_or_else(|e| {
                error!("Error parsing visualization server socket address: {}", e);
                process::exit(1);
            });
            info!("Starting visualization server at {}", &addr);
            VisualizationServer::start(addr, &blockchain, &blockdb, &utxodb);
        }
        None => {}
    }

    loop {
        std::thread::park();
    }
}
