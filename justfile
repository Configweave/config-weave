# config-weave command runner

# Where `just install` puts the per-platform binaries (override: `just prefix=… install`).
data_dir := env_var_or_default("XDG_DATA_HOME", env_var("HOME") / ".local/share")
prefix   := data_dir / "config-weave/bin"
bin_dir  := env_var("HOME") / ".local/bin"

# Fixed dev-server addresses so the two docs sites (and other projects on the
# default 8080) never collide. Must match DOCS_ADDR in ../config-weave-pkgs.
docs_addr      := "127.0.0.1:8280"
pkgs_docs_addr := "127.0.0.1:8281"

[default, private]
main:
	@just --list

# Build the debug binary
[group('build')]
build:
	cargo build

# Run the full test suite (the CLI + the weave-docjson crate; the
# default-members setting would otherwise skip the latter).
[group('test')]
test:
	cargo test
	cargo test -p weave-docjson

# Lint and format checks
[group('test')]
check:
	cargo clippy -- -D warnings
	cargo fmt --check

# Validate the sample playbook with the debug binary
[group('test')]
sample: build
	cargo run -q -- validate testdata/sample

# Docker-backed testlab suite: cross-builds the static binary the tests
# copy into containers, then runs the #[ignore]-gated tests. Needs
# docker (or podman) and `cross`.
[group('test'), doc("Docker-backed testlab suite (#[ignore]-gated; needs docker + cross)")]
test-lab:
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CONFIG_WEAVE_TEST_BINARY=$(realpath target-cross/x86_64-unknown-linux-musl/release/config-weave) \
		cargo test --test testlab -- --ignored

# vmlab-backed testlab smoke: cross-builds the static binary, then runs
# every package test in disposable VMs cloned from the given template.
# Needs vmlab, KVM, and a built template (see ../vmlab).
[group('test'), doc("vmlab-backed testlab smoke in disposable VMs (needs vmlab + KVM + template)")]
test-lab-vm dir='../config-weave-pkgs' template='x86_64/ubuntu-24.04': build
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CONFIG_WEAVE_TEST_BINARY=$(realpath target-cross/x86_64-unknown-linux-musl/release/config-weave) \
		target/debug/config-weave test {{dir}} --backend vmlab --image {{template}}

# Run the windows_domain AD scenario (full DC lifecycle over real reboots).
# Heavy: provisions several windows-server-2025 VMs. Needs `cross`, vmlab, KVM,
# and the x86_64/windows-server-2025 template in the store.
[group('test'), doc("windows_domain AD scenario, full DC lifecycle (heavy; needs cross + vmlab + KVM)")]
test-ad: build
	test -d ../config-weave-pkgs
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-pc-windows-gnu
	target/debug/config-weave test ../config-weave-pkgs windows_domain:ad_matrix \
		--binary target-cross/x86_64-unknown-linux-musl/release/config-weave \
		--binary-windows target-cross/x86_64-pc-windows-gnu/release/config-weave.exe \
		--vmlab-jobs 1

# Build config-weave and run the sibling standard package library checks.
# Each test runs on its own declared backend, so this needs ../config-weave-pkgs,
# `cross`, docker (or podman) for the linux/docker tests, and vmlab + KVM + a
# built windows template for the vmlab/windows tests.
[group('test'), doc("Run the sibling standard package library checks on their declared backends")]
test-pkgs: build
	test -d ../config-weave-pkgs
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-pc-windows-gnu
	target/debug/config-weave wscripti ../config-weave-pkgs
	target/debug/config-weave validate ../config-weave-pkgs
	target/debug/config-weave test ../config-weave-pkgs \
		--binary target-cross/x86_64-unknown-linux-musl/release/config-weave \
		--binary-windows target-cross/x86_64-pc-windows-gnu/release/config-weave.exe
	target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs --pkg-only

# Build config-weave, render the sibling package docs, and serve them with
# WCL's own watch-rebuild dev server (live reload). Needs `wcl` on PATH.
[group('docs'), doc("Render + serve the sibling package docs with live reload (needs wcl)")]
serve-pkgs-docs: build
	test -d ../config-weave-pkgs
	target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs --pkg-only --serve --addr {{pkgs_docs_addr}}

# Serve config-weave's own documentation site (landing at /, the config-weave
# reference book under /wskills/config-weave/) with live reload and comment mode
# (click a rendered block to leave a review note in a comments.wcl sidecar; list
# them with `wcl wdoc comments`). Needs `wcl` on PATH.
[group('docs'), doc("Serve config-weave's documentation site with live reload + comment mode (needs wcl)")]
docs-serve *ARGS:
	wcl wdoc serve docs/main.wcl --comment --addr {{docs_addr}} {{ARGS}}

# Build config-weave's documentation site into docs/_site/ (gitignored). Needs `wcl`.
[group('docs')]
docs-build *ARGS:
	wcl wdoc build docs/main.wcl --out docs/_site {{ARGS}}

# Serve config-weave's documentation site and open the landing page in the
# browser once the server responds. Needs `wcl` on PATH.
[group('docs'), doc("Serve the docs site and open the landing page in the browser (needs wcl)")]
docs-open *ARGS: (browser-open "http://" + docs_addr + "/") (docs-serve ARGS)

# Wait for `url` to respond, then open it in the default browser. Backgrounds
# itself so a blocking server recipe can run as the next dependency.
[private]
browser-open url:
	@( for _ in $(seq 1 60); do curl -sf -o /dev/null '{{url}}' && break; sleep 0.5; done; xdg-open '{{url}}' ) >/dev/null 2>&1 &

# Regenerate the committed Claude Code skill (.claude/skills/config-weave/) from the
# config-weave wskill (docs/wskills/config-weave/). Cleans first — `wcl wdoc skill`
# only writes the pages it generates, so stale pages would otherwise linger.
[group('docs'), doc("Regenerate the committed Claude Code skill from the config-weave wskill")]
skill-build *ARGS:
	rm -rf .claude/skills/config-weave
	wcl wdoc skill docs/wskills/config-weave/wdoc/skill/main.wcl --out .claude/skills/config-weave {{ARGS}}

# Release artifacts for both PRD targets plus a checksums file.
# Requires `cross` and a container runtime; path deps are mounted into
# the build container (see Cross.toml).
[group('build'), doc("Cross-build release artifacts for both PRD targets + checksums")]
release:
	# Separate CARGO_TARGET_DIRs: cross runs each target in its own container,
	# and host-arch build scripts compiled under one image's glibc fail to run
	# under the other's ("GLIBC_x.yz not found" — seen on CI runners).
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross/musl \
		cross build --release --target x86_64-unknown-linux-musl
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CARGO_TARGET_DIR=target-cross/win \
		cross build --release --target x86_64-pc-windows-gnu
	mkdir -p dist
	cp target-cross/musl/x86_64-unknown-linux-musl/release/config-weave dist/config-weave-linux-x86_64
	cp target-cross/win/x86_64-pc-windows-gnu/release/config-weave.exe dist/config-weave-windows-x86_64.exe
	cd dist && sha256sum config-weave-linux-x86_64 config-weave-windows-x86_64.exe > SHA256SUMS
	@echo "release artifacts in dist/"

# Cross-build both supported platforms and install their binaries into a single
# folder ({{prefix}}), plus a `config-weave` symlink on PATH ({{bin_dir}}).
# Reuses `release` for the cross-builds. Requires `cross` + a container runtime.
[group('build'), doc("Cross-build and install both platform binaries + a PATH symlink")]
install: release
	mkdir -p {{prefix}}
	cp dist/config-weave-linux-x86_64 dist/config-weave-windows-x86_64.exe dist/SHA256SUMS {{prefix}}/
	mkdir -p {{bin_dir}}
	ln -sf {{prefix}}/config-weave-linux-x86_64 {{bin_dir}}/config-weave
	@echo "installed platform binaries to {{prefix}}"
	@echo "linked {{bin_dir}}/config-weave -> config-weave-linux-x86_64"
	@echo "ensure {{bin_dir}} is on your PATH"
