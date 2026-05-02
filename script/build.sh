#!/usr/bin/env bash

set -euo pipefail
cargo build --release --target x86_64-unknown-linux-musl --package=exvlan-api --features=embed
mv ./target/x86_64-unknown-linux-musl/release/exvlan-api release/exvlan-api-embed
cargo build --release --target x86_64-unknown-linux-musl --features jemalloc
mv ./target/x86_64-unknown-linux-musl/release/exvlan release/exvlan
mv ./target/x86_64-unknown-linux-musl/release/exvlan-api release/exvlan-api
mv ./target/x86_64-unknown-linux-musl/release/exvlan-cli release/exvlan-cli