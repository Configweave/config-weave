# config-weave command runner

# Where `just install` puts the per-platform binaries (override: `just prefix=… install`).
data_dir := env_var_or_default("XDG_DATA_HOME", env_var("HOME") / ".local/share")
prefix   := data_dir / "config-weave/bin"
bin_dir  := env_var("HOME") / ".local/bin"

# Fixed dev-server addresses so the two docs sites (and other projects on the
# default 8080) never collide. Must match DOCS_ADDR in config-weave-pkgs.
docs_addr      := "127.0.0.1:8280"
pkgs_docs_addr := "127.0.0.1:8281"

# Where the standard package library is checked out. Overridable, because a
# ticket worktree (<repo>/.tree/<ticket>) has no sibling checkouts.
pkgs_dir := env_var_or_default("CONFIG_WEAVE_PKGS", "../config-weave-pkgs")

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
	# --all-targets so tests and benches are linted too — CI lints them,
	# and without it a lint in test code only surfaces after a push.
	cargo clippy --all-targets -- -D warnings
	cargo fmt --check

# Validate the sample playbook with the debug binary
[group('test')]
sample: build
	cargo run -q -- validate testdata/sample

# Testlab suite: cross-builds the static binary the tests copy into
# instances, then runs the #[ignore]-gated tests. They provision vmlab
# containers, so this needs vmlab and `cross`.
[group('test'), doc("Testlab suite in vmlab containers (#[ignore]-gated; needs vmlab + cross)")]
test-lab:
	CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CONFIG_WEAVE_TEST_BINARY=$(realpath target-cross/x86_64-unknown-linux-musl/release/config-weave) \
		cargo test --test testlab -- --ignored

# Testlab smoke in full VMs: cross-builds the static binary, then runs
# every VM package test against the given template. Needs vmlab, KVM, and
# a built template (see ../vmlab).
[group('test'), doc("Testlab smoke in disposable VMs (needs vmlab + KVM + template)")]
test-lab-vm dir=pkgs_dir template='x86_64/ubuntu-24.04': build
	CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	target/debug/config-weave test {{dir}} --template {{template}} \
		--binary target-cross/x86_64-unknown-linux-musl/release/config-weave

# Run the windows_domain AD scenario (full DC lifecycle over real reboots).
# Heavy: provisions several windows-server-2025 VMs. Needs `cross`, vmlab, KVM,
# and the x86_64/windows-server-2025 template in the store.
[group('test'), doc("windows_domain AD scenario, full DC lifecycle (heavy; needs cross + vmlab + KVM)")]
test-ad: build
	test -d {{pkgs_dir}}
	CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-pc-windows-gnu
	target/debug/config-weave test {{pkgs_dir}} windows_domain:ad_matrix \
		--binary target-cross/x86_64-unknown-linux-musl/release/config-weave \
		--binary-windows target-cross/x86_64-pc-windows-gnu/release/config-weave.exe \
		--vm-jobs 1

# Build config-weave and run the sibling standard package library checks.
# Every test provisions a vmlab instance — a container for `image` tests, a
# VM for `template` ones — so this needs ../config-weave-pkgs, `cross`,
# vmlab + KVM, and a built windows template for the windows tests.
[group('test'), doc("Run the sibling standard package library checks in vmlab instances")]
test-pkgs: build
	test -d {{pkgs_dir}}
	CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-pc-windows-gnu
	target/debug/config-weave wscripti {{pkgs_dir}}
	target/debug/config-weave validate {{pkgs_dir}}
	target/debug/config-weave test {{pkgs_dir}} \
		--binary target-cross/x86_64-unknown-linux-musl/release/config-weave \
		--binary-windows target-cross/x86_64-pc-windows-gnu/release/config-weave.exe
	target/debug/config-weave docs {{pkgs_dir}} {{pkgs_dir}}/docs --pkg-only

# Build config-weave, render the sibling package docs, and serve them with
# WCL's own dev server. It watches for `.wcl` changes but does not rebuild
# on its own — press Enter in the console to rebuild. Needs `wcl` on PATH.
[group('docs'), doc("Render + serve the sibling package docs (needs wcl)")]
serve-pkgs-docs: build
	test -d {{pkgs_dir}}
	target/debug/config-weave docs {{pkgs_dir}} {{pkgs_dir}}/docs --pkg-only --serve --addr {{pkgs_docs_addr}}

# Serve config-weave's own documentation site (landing at /, the config-weave
# reference book under /wskills/config-weave/). Watches for `.wcl` changes but
# does not rebuild on its own — press Enter in the console. Needs `wcl` on PATH.
[group('docs'), doc("Serve config-weave's documentation site (needs wcl)")]
docs-serve *ARGS:
	wcl wdoc serve docs/main.wcl --addr {{docs_addr}} {{ARGS}}

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
# Requires `cross` and a container runtime; the cross-repo deps are fetched
# from GitHub inside the container (see Cross.toml).
[group('build'), doc("Cross-build release artifacts for both PRD targets + checksums")]
release:
	# Separate CARGO_TARGET_DIRs: cross runs each target in its own container,
	# and host-arch build scripts compiled under one image's glibc fail to run
	# under the other's ("GLIBC_x.yz not found" — seen on CI runners).
	CARGO_TARGET_DIR=target-cross/musl \
		cross build --release --target x86_64-unknown-linux-musl
	CARGO_TARGET_DIR=target-cross/win \
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
