#!/usr/bin/env bash
set -euo pipefail

IMAGE="carplay-builder"

HOST_ARCH=$(uname -m)
if [ "$HOST_ARCH" = "arm64" ] || [ "$HOST_ARCH" = "aarch64" ]; then
    PLATFORM="--platform linux/arm64"
    CROSS_COMPILE=false
    BINDIR="target/release"
else
    PLATFORM=""
    CROSS_COMPILE=true
    BINDIR="target/aarch64-unknown-linux-gnu/release"
fi

build_image() {
    echo "Building Docker image (cross=${CROSS_COMPILE})..."
    # shellcheck disable=SC2086
    docker build $PLATFORM --build-arg CROSS_COMPILE="$CROSS_COMPILE" -f Dockerfile.cross -t "$IMAGE" .
}

build_binary() {
    echo "Compiling workspace..."
    # shellcheck disable=SC2086
    docker run --rm -t $PLATFORM \
        -e CARGO_TERM_COLOR=always \
        -e CROSS_COMPILE="$CROSS_COMPILE" \
        -v "$(pwd)":/build \
        -v carplay-cargo-cache:/root/.cargo/registry \
        "$IMAGE"
    echo "Binaries ready:"
    echo "  ${BINDIR}/carplay-audio  (service)"
    echo "  ${BINDIR}/carplay-tui    (TUI client)"
}

deploy() {
    local host="${1:?Usage: $0 deploy <user@host>}"
    scp "${BINDIR}/carplay-audio" "${BINDIR}/carplay-tui" "$host":~/
    echo "Deployed to $host"
}

case "${1:-build}" in
    image)  build_image ;;
    build)  build_binary ;;
    deploy) deploy "${2:-}" ;;
    all)    build_image && build_binary ;;
    *)
        echo "Usage: $0 [image|build|deploy <user@host>|all]"
        echo "  image   -- (re)build the Docker build image"
        echo "  build   -- compile the workspace (default)"
        echo "  deploy  -- copy binaries to the Pi via scp"
        echo "  all     -- image + build"
        exit 1
        ;;
esac
