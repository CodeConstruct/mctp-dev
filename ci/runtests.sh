#!/bin/bash -ex

export CARGO_TARGET_DIR=target/ci

export RUSTDOCFLAGS='-D warnings'
export RUSTFLAGS="-D warnings"

cargo fmt -- --check
cargo build --release
cargo test
cargo doc
