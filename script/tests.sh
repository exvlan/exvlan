#!/usr/bin/env bash

set -euo pipefail

main() {
    if [ "$(id -u)" -eq 0 ]; then
        echo "Setting up system for test..."
        modprobe br_netfilter 2>/dev/null || true
        sysctl net.bridge.bridge-nf-call-iptables=0 2>/dev/null || true
        sysctl net.bridge.bridge-nf-call-ip6tables=0 2>/dev/null || true
        sysctl net.ipv6.conf.lo.disable_ipv6=0 2>/dev/null || true
        ip addr add 2001:db8::2/64 dev lo 2>/dev/null || true

        echo "Configuring test domains in /etc/hosts..."
        echo "127.0.0.1 test.xvlan.era-x.cn" >> /etc/hosts
        echo "::1 test.xvlan.era-x.cn" >> /etc/hosts
        echo "127.0.0.1 github-ci-test.xvlan.era-x.cn" >> /etc/hosts
        echo "::1 github-ci-test.xvlan.era-x.cn" >> /etc/hosts
        echo "127.0.0.1 txt.xvlan.era-x.cn" >> /etc/hosts
        echo "::1 txt.xvlan.era-x.cn" >> /etc/hosts
        echo "127.0.0.1 txt.stun.xvlan.era-x.cn" >> /etc/hosts
        echo "::1 txt.stun.xvlan.era-x.cn" >> /etc/hosts
        echo "127.0.0.1 txt.stun-v6.xvlan.era-x.cn" >> /etc/hosts
        echo "::1 txt.stun-v6.xvlan.era-x.cn" >> /etc/hosts
    fi

    rustup component add rustfmt
    rustup component add clippy

    cargo fmt --all
    cargo fmt --all -- --check

    cargo clippy --all-targets --features full --all -- -D warnings

    cargo hack check --package exvlan --each-feature --exclude-features macos-ne --verbose

    if ! cargo metadata --format-version 1 --locked > /dev/null; then
        echo "::error::Cargo.lock is out of date. Run cargo generate-lockfile or cargo build locally, then commit Cargo.lock."
        exit 1
    fi

    mkdir -p /workspace/archive

    curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin

    ARCHIVE_FILE="/workspace/archive/tests.tar.zst"
    CARGO_TOML="/workspace/Cargo.toml"
    CARGO_LOCK="/workspace/Cargo.lock"

    if [ -f "$ARCHIVE_FILE" ]; then
        ARCHIVE_MTIME=$(stat -c %Y "$ARCHIVE_FILE")
        CARGO_TOML_MTIME=$(stat -c %Y "$CARGO_TOML")
        CARGO_LOCK_MTIME=$(stat -c %Y "$CARGO_LOCK")

        if [ "$ARCHIVE_MTIME" -gt "$CARGO_TOML_MTIME" ] && [ "$ARCHIVE_MTIME" -gt "$CARGO_LOCK_MTIME" ]; then
            echo "Using cached archive: $ARCHIVE_FILE"
        else
            echo "Cargo.toml or Cargo.lock changed, rebuilding archive..."
            cargo nextest archive --archive-file "$ARCHIVE_FILE" --package exvlan --features full
        fi
    else
        echo "Archive not found, creating new archive..."
        cargo nextest archive --archive-file "$ARCHIVE_FILE" --package exvlan --features full
    fi

    prlimit --pid $$ --nofile=1048576:1048576 2>/dev/null || true

    echo "=== Running exvlan tests (excluding three_node) ==="
    cargo nextest run --archive-file "$ARCHIVE_FILE" -E 'not test(tests::three_node)' --test-threads 1 --no-fail-fast 2>&1 | grep -E '(FAIL|error|Summary|---|test\()' || true

    echo ""
    echo "=== Running three_node tests ==="
    cargo nextest run --archive-file "$ARCHIVE_FILE" -E 'test(tests::three_node) and not test(subnet_proxy_three_node_test)' --test-threads 1 --no-fail-fast 2>&1 | grep -E '(FAIL|error|Summary|---|test\()' || true

    echo ""
    echo "=== Running subnet_proxy_three_node_test ==="
    cargo nextest run --archive-file "$ARCHIVE_FILE" -E 'test(subnet_proxy_three_node_test)' --test-threads 1 --no-fail-fast 2>&1 | grep -E '(FAIL|error|Summary|---|test\()' || true
}

main "$@"