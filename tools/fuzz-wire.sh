#!/usr/bin/env bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
nightly=nightly-2026-11-10
fuzz_version=0.13.2
rustup run "$nightly" rustc --version >/dev/null 2>&1 ||
    rustup toolchain install "$nightly" --profile minimal
if [[ "$(cargo +"$nightly" fuzz --version 2>/dev/null || true)" != "cargo-fuzz $fuzz_version" ]]; then
    cargo install cargo-fuzz --version "$fuzz_version" --locked
fi
cargo fmt --manifest-path fuzz/Cargo.toml --check
cargo +"$nightly" metadata --manifest-path fuzz/Cargo.toml --locked --format-version 1 >/dev/null

corpus=$(mktemp -d)
trap 'rm -rf -- "$corpus"' EXIT
cp fuzz/corpus/wire/* "$corpus/"
limits=(-runs=1000 -seed=42)
if [[ -n ${FUZZ_SECONDS:-} ]]; then
    limits=("-max_total_time=$FUZZ_SECONDS")
fi
cargo +"$nightly" fuzz run wire "$corpus" -- "${limits[@]}" -max_len=32768
