#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
mode=${1:-host}
if [[ "$mode" == host ]]; then
    cargo build --locked -p fixtester-desktop --example native_smoke
    timeout 20 "${CARGO_TARGET_DIR:-target}/debug/examples/native_smoke"
    exit
fi

if [[ "$mode" != alma ]]; then
    echo "Usage: $0 [host|alma]" >&2
    exit 2
fi

tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
toolchain=$(dirname "$(dirname "$(rustup which rustc)")")
cargo_cache=${CARGO_HOME:-$HOME/.cargo}
container=${FIXTESTER_CONTAINER:-podman}

"$container" build -t localhost/fixtester-smoke-build:8 -f - "$tmp" <<'DOCKERFILE'
FROM docker.io/library/almalinux:8
RUN dnf -y install gcc && dnf clean all
DOCKERFILE

"$container" run --rm --network=host \
    -v "$PWD:/src:ro" -v "$toolchain:/rust:ro" -v "$cargo_cache/registry:/cargo/registry" \
    -v "$tmp:/output" -w /src -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/output \
    -e PATH=/rust/bin:/usr/bin:/bin localhost/fixtester-smoke-build:8 \
    cargo build --locked --release -p fixtester-desktop --example native_smoke

binary=$tmp/release/examples/native_smoke
max_glibc=$(objdump -T "$binary" | sed -n 's/.*(GLIBC_\([0-9.]*\)).*/\1/p' | sort -V | tail -n 1)
if [[ -z "$max_glibc" || $(printf '%s\n' "$max_glibc" 2.28 | sort -V | tail -n 1) != 2.28 ]]; then
    echo "Unexpected maximum glibc symbol version: $max_glibc" >&2
    exit 1
fi
echo "Maximum glibc symbol version: $max_glibc"

for version in 8 9; do
    image="localhost/fixtester-smoke-display:$version"
    "$container" build -t "$image" -f - "$tmp" <<DOCKERFILE
FROM docker.io/library/almalinux:$version
RUN dnf -y install xorg-x11-server-Xvfb mesa-libEGL mesa-libGL mesa-dri-drivers libxkbcommon libXcursor libXi libXrandr libXinerama && dnf clean all
DOCKERFILE
    "$container" run --rm -v "$binary:/native-smoke:ro" "$image" \
        bash -ec 'cat /etc/almalinux-release
            Xvfb :88 -screen 0 1024x768x24 -nolisten tcp &
            xvfb=$!
            trap "kill $xvfb" EXIT
            for ((retry = 0; retry < 50; retry++)); do
                [[ -S /tmp/.X11-unix/X88 ]] && break
                sleep 0.1
            done
            DISPLAY=:88 LIBGL_ALWAYS_SOFTWARE=1 timeout 20 /native-smoke'
done
