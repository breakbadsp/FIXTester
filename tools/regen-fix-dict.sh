#!/usr/bin/env bash
# Regenerates crates/fix-dict/src/generated/ from the FIX Unified Repository.
#
# It downloads the ZIP, checks its SHA-256 against the pinned value, extracts
# FixRepository.xml, and runs the generator. The output is rustfmt clean.
# Set FIXSIM_REPOSITORY_ZIP to a local copy of the ZIP to skip the download.
set -euo pipefail

URL='https://fixtrading.org/packages/fix-4-0-fix-5-0-sp2-unified-repository/?wpdmdl=2323'
SHA256='54c498a82cb40962fdbd25cd74f0594af0a296aa80daa45f0285c2cacbde55f7'
ENTRY='fix_repository_2010_edition_20200402/Unified/FixRepository.xml'

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

zip="${FIXSIM_REPOSITORY_ZIP:-}"
if [ -z "$zip" ]; then
    zip="$work/repository.zip"
    curl --fail --silent --show-error --location --output "$zip" "$URL"
fi

actual="$(sha256sum "$zip" | cut -d' ' -f1)"
if [ "$actual" != "$SHA256" ]; then
    echo "regen-fix-dict: SHA-256 of the ZIP is $actual, expected $SHA256" >&2
    exit 1
fi

mkdir "$work/xml"
unzip -q "$zip" "$ENTRY" -d "$work/xml"

cd "$root"
cargo run --quiet --locked -p fix-dict-gen -- \
    --input "$work/xml/$ENTRY" \
    --out-dir crates/fix-dict/src/generated \
    --source-sha256 "$SHA256"
