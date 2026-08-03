#!/usr/bin/env sh
set -e
exec cargo run --release --quiet -p xtask -- bundle --release --target aarch64-apple-darwin
