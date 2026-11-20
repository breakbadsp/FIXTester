#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
mode=${1:-host}
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
if [[ "$mode" == host ]]; then
    echo "Host session: ${XDG_CURRENT_DESKTOP:-unknown} (${XDG_SESSION_TYPE:-unknown})"
    if [[ -n "${2:-}" ]]; then
        timeout 20 "$2" --smoke --data-dir "$tmp"
    else
        cargo build --locked -p fixtester-desktop --bin fixtester
        timeout 20 cargo run --locked -p fixtester-desktop --bin fixtester -- --smoke --data-dir "$tmp"
    fi
    exit
fi

if [[ "$mode" != alma ]]; then
    echo "Usage: $0 [host [binary] | alma [release-output]]" >&2
    exit 2
fi

toolchain=$(dirname "$(dirname "$(rustup which rustc)")")
cargo_cache=${CARGO_HOME:-$HOME/.cargo}
container=${FIXTESTER_CONTAINER:-podman}
image=localhost/fixtester-smoke-display:8
mkdir -p "$cargo_cache/registry" "$tmp/tests"

"$container" build -t localhost/fixtester-smoke-build:8 -f - "$tmp" <<'DOCKERFILE'
FROM docker.io/library/almalinux:8
RUN dnf -y install gcc && dnf clean all
DOCKERFILE

"$container" run --rm --network=host \
    -v "$PWD:/src:ro" -v "$toolchain:/rust:ro" -v "$cargo_cache/registry:/cargo/registry" \
    -v "$tmp:/output" -w /src -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/output -e TMPDIR=/output/tests \
    -e PATH=/rust/bin:/usr/bin:/bin localhost/fixtester-smoke-build:8 \
    bash -ec 'cargo build --locked --release -p fixtester-desktop --bin fixtester
        cargo test --locked --release -p fixtester-desktop'

binary=$tmp/release/fixtester
max_glibc=$(objdump -T "$binary" | sed -n 's/.*(GLIBC_\([0-9.]*\)).*/\1/p' | sort -V | tail -n 1)
if [[ -z "$max_glibc" || $(printf '%s\n' "$max_glibc" 2.28 | sort -V | tail -n 1) != 2.28 ]]; then
    echo "Unexpected maximum glibc symbol version: $max_glibc" >&2
    exit 1
fi
echo "Maximum glibc symbol version: $max_glibc"
objdump -p "$binary" | awk '$1 == "NEEDED" {print $2}'
sha256sum "$binary"

"$container" build -t "$image" -f - "$tmp" <<'DOCKERFILE'
FROM docker.io/library/almalinux:8
RUN dnf -y install xorg-x11-server-Xvfb mesa-libEGL mesa-libGL mesa-dri-drivers libxkbcommon-x11 libXcursor libXi libXrandr libXinerama && dnf clean all
DOCKERFILE

"$container" run --rm -v "$binary:/fixtester:ro" -v "$tmp:/smoke" -e TMPDIR=/smoke "$image" \
    bash -ec 'cat /etc/almalinux-release
        Xvfb :88 -screen 0 1280x800x24 -nolisten tcp &
        xvfb=$!
        trap "kill $xvfb" EXIT
        for ((retry = 0; retry < 50; retry++)); do
            [[ -S /tmp/.X11-unix/X88 ]] && break
            sleep 0.1
        done
        DISPLAY=:88 LIBGL_ALWAYS_SOFTWARE=1 timeout 20 /fixtester --smoke --data-dir /smoke/data'
if [[ -n "${2:-}" ]]; then
    install -m 0755 "$binary" "$2"
fi
