#!/usr/bin/env bash
set -euo pipefail

spec=$(realpath -- "${1:?Usage: tools/check-tla.sh PATH.tla}")
[[ "$spec" == *.tla && -f "$spec" && -f "${spec%.tla}.cfg" ]] || {
    echo 'Expected a TLA+ module and a config with the same name.' >&2
    exit 2
}
module=$(basename -- "$spec" .tla)
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

jar=${TLA2TOOLS:-"$scratch/tla2tools.jar"}
if [[ -z ${TLA2TOOLS:-} ]]; then
    curl --fail --silent --show-error --location --retry 3 --output "$jar" \
        https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
fi
printf '%s  %s\n' 936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88 "$jar" |
    sha256sum --check --status || { echo 'TLC v1.7.4 checksum mismatch.' >&2; exit 1; }
jar=$(realpath -- "$jar")

cp -- "$spec" "${spec%.tla}.cfg" "$scratch"
cd -- "$scratch"
"${JAVA:-java}" -XX:+UseParallelGC -Xmx512m -cp "$jar" tlc2.TLC -workers 1 "$module"
