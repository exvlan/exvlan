#!/usr/bin/env bash

set -euo pipefail

main() {
    # Install rustfmt and clippy
    rustup component add rustfmt
    rustup component add clippy
    # Check formatting
    cargo fmt --all 
    cargo fmt --all -- --check
    # Check Clippy
    cargo clippy --all-targets --features full --all -- -D warnings
    # Check features
    cargo hack check --package exvlan --each-feature --exclude-features macos-ne --verbose
    # cargo clippy --all-targets --features full --all
    # setup nextest
    curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    # Archive test
    cargo nextest archive --archive-file tests.tar.zst --package exvlan --features full
    # Run test exvlan
    cargo nextest run --archive-file tests.tar.zst -E 'not test(tests::three_node)' --test-threads 1 --no-fail-fast
    # Run test three_node
    cargo nextest run --archive-file tests.tar.zst -E 'test(tests::three_node) and not test(subnet_proxy_three_node_test)' --test-threads 1 --no-fail-fast  
    # Run test subnet_proxy_three_node_test
    cargo nextest run --archive-file tests.tar.zst -E 'test(subnet_proxy_three_node_test)' --test-threads 1 --no-fail-fast  
}

main "$@"