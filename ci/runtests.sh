#!/bin/bash -ex

export CARGO_TARGET_DIR=target/ci

export RUSTDOCFLAGS='-D warnings'
export RUSTFLAGS="-D warnings"

cargo fmt -- --check

# base checks
cargo check --locked
cargo build --release
cargo doc
cargo test

# features
sets=(
    ""
    "nvme-mi"
)

for features in "${sets[@]}"
do
    feature_args='--no-default-features'
    if [ ${#features} -gt 0 ]
    then
        feature_args="$feature_args --features $features"
    fi
    cargo build $feature_args
    cargo clippy $feature_args
done
