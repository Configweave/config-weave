# config-weave command runner

# Where `just install` puts the per-platform binaries (override: `just prefix=… install`).
data_dir := env_var_or_default("XDG_DATA_HOME", env_var("HOME") / ".local/share")
prefix   := data_dir / "config-weave/bin"
bin_dir  := env_var("HOME") / ".local/bin"

[default, private]
main:
    @just --list

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

# Run the windows_domain AD scenario (full DC lifecycle over real reboots).
# Heavy: provisions several windows-server-2025 VMs. Needs `cross`, vmlab, KVM,
# and the x86_64/windows-server-2025 template in the store.
test-ad: build
    test -d ../config-weave-pkgs
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-unknown-linux-musl
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-pc-windows-gnu
    target/debug/config-weave test ../config-weave-pkgs windows_domain:ad_matrix \
        --binary target-cross/x86_64-unknown-linux-musl/release/config-weave \
        --binary-windows target-cross/x86_64-pc-windows-gnu/release/config-weave.exe \
        --vmlab-jobs 1

# Build config-weave and run the sibling standard package library checks.
# Each test runs on its own declared backend, so this needs ../config-weave-pkgs,
# `cross`, docker (or podman) for the linux/docker tests, and vmlab + KVM + a
# built windows template for the vmlab/windows tests.
test-pkgs: build
    test -d ../config-weave-pkgs
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-unknown-linux-musl
    CW_WCL=$(realpath ../WCL) CW_WISP=$(realpath ../wisp) CARGO_TARGET_DIR=target-cross \
        cross build --release --target x86_64-pc-windows-gnu
    target/debug/config-weave wispi ../config-weave-pkgs
    target/debug/config-weave validate ../config-weave-pkgs
    target/debug/config-weave test ../config-weave-pkgs \
        --binary target-cross/x86_64-unknown-linux-musl/release/config-weave \
        --binary-windows target-cross/x86_64-pc-windows-gnu/release/config-weave.exe
    target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs

# Build config-weave, render the sibling package docs, and serve them with
# WCL's own watch-rebuild dev server (live reload). Needs `wcl` on PATH.
serve-pkgs-docs: build
    test -d ../config-weave-pkgs
    target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs
    @echo "serving package docs at http://127.0.0.1:8080"
    wcl wdoc serve ../config-weave-pkgs/docs/_weave_docs.wcl

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

# Cross-build both supported platforms and install their binaries into a single
# folder ({{prefix}}), plus a `config-weave` symlink on PATH ({{bin_dir}}).
# Reuses `release` for the cross-builds. Requires `cross` + a container runtime.
install: release
    mkdir -p {{prefix}}
    cp dist/config-weave-linux-x86_64 dist/config-weave-windows-x86_64.exe dist/SHA256SUMS {{prefix}}/
    mkdir -p {{bin_dir}}
    ln -sf {{prefix}}/config-weave-linux-x86_64 {{bin_dir}}/config-weave
    @echo "installed platform binaries to {{prefix}}"
    @echo "linked {{bin_dir}}/config-weave -> config-weave-linux-x86_64"
    @echo "ensure {{bin_dir}} is on your PATH"
