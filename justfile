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

# Docker-backed testlab suite: cross-builds the static binary the tests
# copy into containers, then runs the #[ignore]-gated tests. Needs
# docker (or podman) and `cross`.
test-lab:
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-unknown-linux-musl
    CONFIG_WEAVE_TEST_BINARY=$(realpath target-cross/x86_64-unknown-linux-musl/release/config-weave) \
        cargo test --test testlab -- --ignored

# vmlab-backed testlab smoke: cross-builds the static binary, then runs
# every package test in disposable VMs cloned from the given template.
# Needs vmlab, KVM, and a built template (see ../vmlab).
test-lab-vm dir='../config-weave-pkgs' template='x86_64/ubuntu-24.04': build
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-unknown-linux-musl
    CONFIG_WEAVE_TEST_BINARY=$(realpath target-cross/x86_64-unknown-linux-musl/release/config-weave) \
        target/debug/config-weave test {{dir}} --backend vmlab --image {{template}}

# Build config-weave and run the sibling standard package library checks.
# Needs ../config-weave-pkgs plus docker (or podman) for package tests.
test-pkgs: build
    test -d ../config-weave-pkgs
    target/debug/config-weave wispi ../config-weave-pkgs
    target/debug/config-weave validate ../config-weave-pkgs
    target/debug/config-weave test ../config-weave-pkgs
    target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs

# Build config-weave, render the sibling package docs, and serve them.
serve-pkgs-docs: build
    test -d ../config-weave-pkgs
    target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs
    @echo "serving package docs at http://127.0.0.1:8000"
    cd ../config-weave-pkgs/docs && python3 -m http.server 8000 --bind 127.0.0.1

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
