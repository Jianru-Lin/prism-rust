# Prism: Scaling Bitcoin by 10,000x

[![Build Status](https://travis-ci.com/yangl1996/prism-rust.svg?token=HUqJJ7ZuGW1FW5vCJpjM&branch=master)](https://travis-ci.com/yangl1996/prism-rust)

Rust implementation of the Prism consensus protocol.

## Paper

__Prism: Scaling Bitcoin by 10,000x__ [\[full text\]](https://arxiv.org/pdf/1909.11261.pdf)

_[Lei Yang](http://leiy.me) (MIT CSAIL), [Vivek Bagaria](https://www.linkedin.com/in/vivek-bagaria-7a833637/) [📧](mailto:vbagaria@stanford.edu) (Stanford University), [Gerui Wang](https://www.linkedin.com/in/gerui-wang-495736a3/) (UIUC) [📧](mailto:geruiw2@illinois.edu), [Mohammad Alizadeh](http://people.csail.mit.edu/alizadeh/) (MIT CSAIL), [David Tse](https://tselab.stanford.edu/people/principal-investigator/david-tse/) (Stanford University), [Giulia Fanti](https://www.andrew.cmu.edu/user/gfanti/) (CMU), [Pramod Viswanath](http://pramodv.ece.illinois.edu) (UIUC)_

Abstract: Bitcoin is the first fully decentralized permissionless blockchain protocol and achieves a high level of security: the ledger it maintains has guaranteed liveness and consistency properties as long as the adversary has less compute power than the honest nodes. However, its throughput is only 7 transactions per second and the confirmation latency can be up to hours. Prism is a new blockchain protocol which is designed to achieve a natural scaling of Bitcoin's performance while maintaining its full security guarantees. We present an implementation of Prism which achieves a throughput of 70,000 transactions per second and confirmation latencies of tens of seconds.

## Build

This project requires Rust `nightly` because some of our dependencies rely on nightly-only features like inline assembly. However, as of Oct. 31 2019, the cutting-edge `nightly` toolchain won't build the project due to incompatibility in `rocksdb-rust` (see [here](https://github.com/rust-rocksdb/rust-rocksdb/issues/335) and [here](https://github.com/rust-rocksdb/rust-rocksdb/issues/341)).

As a workaround, please use `nightly-2019-09-13` (install by `rustup toolchain install nightly-2019-09-13`) for now.

To build the binary, run `cargo build --release`.

The first build could take several mintues, mostly due to building RocksDB.

## Testbed and Reproducing

The scripts used in the evaluation section of the paper are located in `/testbed`. `/testbed/README.md` provides instructions for running the experiments and reproducing the results. Or, watch the screencast for a quick demo

[![asciicast](https://asciinema.org/a/YGz4dIkfKz4DrHLtVIGSfpmly.svg)](https://asciinema.org/a/YGz4dIkfKz4DrHLtVIGSfpmly)
