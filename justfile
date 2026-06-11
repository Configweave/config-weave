# config-weave command runner

# Build the debug binary
build:
    cargo build

# Run the full test suite
test:
    cargo test

# Lint and format checks
check:
    cargo clippy -- -D warnings
    cargo fmt --check

# Validate the sample playbook with the debug binary
sample: build
    cargo run -q -- validate testdata/sample

# Release artifacts for both PRD targets plus a checksums file.
# Requires `cross` and a container runtime; path deps are mounted into
# the build container (see Cross.toml).
release:
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-unknown-linux-musl
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-pc-windows-gnu
    mkdir -p dist
    cp target-cross/x86_64-unknown-linux-musl/release/config-weave dist/config-weave-linux-x86_64
    cp target-cross/x86_64-pc-windows-gnu/release/config-weave.exe dist/config-weave-windows-x86_64.exe
    cd dist && sha256sum config-weave-linux-x86_64 config-weave-windows-x86_64.exe > SHA256SUMS
    @echo "release artifacts in dist/"
