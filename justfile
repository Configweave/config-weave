# config-weave command runner

# Where `just install` puts the per-platform binaries (override: `just prefix=… install`).
data_dir := env_var_or_default("XDG_DATA_HOME", env_var("HOME") / ".local/share")
prefix   := data_dir / "config-weave/bin"
bin_dir  := env_var("HOME") / ".local/bin"

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
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CONFIG_WEAVE_TEST_BINARY=$(realpath target-cross/x86_64-unknown-linux-musl/release/config-weave) \
		cargo test --test testlab -- --ignored

# vmlab-backed testlab smoke: cross-builds the static binary, then runs
# every package test in disposable VMs cloned from the given template.
# Needs vmlab, KVM, and a built template (see ../vmlab).
[group('test'), doc("vmlab-backed testlab smoke in disposable VMs (needs vmlab + KVM + template)")]
test-lab-vm dir='../config-weave-pkgs' template='x86_64/ubuntu-24.04': build
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CONFIG_WEAVE_TEST_BINARY=$(realpath target-cross/x86_64-unknown-linux-musl/release/config-weave) \
		target/debug/config-weave test {{dir}} --backend vmlab --image {{template}}

# Run the windows_domain AD scenario (full DC lifecycle over real reboots).
# Heavy: provisions several windows-server-2025 VMs. Needs `cross`, vmlab, KVM,
# and the x86_64/windows-server-2025 template in the store.
[group('test'), doc("windows_domain AD scenario, full DC lifecycle (heavy; needs cross + vmlab + KVM)")]
test-ad: build
	test -d ../config-weave-pkgs
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
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
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-pc-windows-gnu
	target/debug/config-weave wscripti ../config-weave-pkgs
	target/debug/config-weave validate ../config-weave-pkgs
	target/debug/config-weave test ../config-weave-pkgs \
		--binary target-cross/x86_64-unknown-linux-musl/release/config-weave \
		--binary-windows target-cross/x86_64-pc-windows-gnu/release/config-weave.exe
	target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs

# Build config-weave, render the sibling package docs, and serve them with
# WCL's own watch-rebuild dev server (live reload). Needs `wcl` on PATH.
[group('docs'), doc("Render + serve the sibling package docs with live reload (needs wcl)")]
serve-pkgs-docs: build
	test -d ../config-weave-pkgs
	target/debug/config-weave docs ../config-weave-pkgs ../config-weave-pkgs/docs
	@echo "serving package docs at http://127.0.0.1:8080"
	wcl wdoc serve ../config-weave-pkgs/docs/_weave_docs.wcl

# Serve config-weave's own documentation site (landing at /, the config-weave
# reference book under /wskills/config-weave/) with live reload and comment mode
# (click a rendered block to leave a review note in a comments.wcl sidecar; list
# them with `wcl wdoc comments`). Needs `wcl` on PATH.
[group('docs'), doc("Serve config-weave's documentation site with live reload + comment mode (needs wcl)")]
docs-serve *ARGS:
	wcl wdoc serve docs/main.wcl --comment {{ARGS}}

# Build config-weave's documentation site into docs/_site/ (gitignored). Needs `wcl`.
[group('docs')]
docs-build *ARGS:
	wcl wdoc build docs/main.wcl --out docs/_site {{ARGS}}

# Regenerate the committed Claude Code skill (.claude/skills/config-weave/) from the
# config-weave wskill (docs/wskills/config-weave/). Cleans first — `wcl wdoc skill`
# only writes the pages it generates, so stale pages would otherwise linger.
[group('docs'), doc("Regenerate the committed Claude Code skill from the config-weave wskill")]
skill-build *ARGS:
	rm -rf .claude/skills/config-weave
	wcl wdoc skill docs/wskills/config-weave/wdoc/skill/main.wcl --out .claude/skills/config-weave {{ARGS}}

# ------------------------------------------------------------- web GUI

# Build the web UI (SolidJS). The @forge/* deps are `link:`s into the
# sibling ../forge monorepo — build forge once (`just build` there) first.
[group('web'), doc("Build the web UI (SolidJS; needs the sibling ../forge built once)")]
web-build:
	cd web-ui && pnpm install && pnpm build

# Build the weave-server release binary with the frontend embedded.
[group('web')]
server-build: web-build
	cargo build --release -p weave-server

# Build the config-weave-pipeline daemon (headless — no frontend).
[group('web')]
pipeline-build:
	cargo build --release -p config-weave-pipeline

# Spin up the web GUI at http://localhost:8765 against a folder of runbooks
# (each child dir has a playbook.wcl). Builds the frontend + server first.
# Docker test runs need the static CLI: uses the one from `just install`
# unless CONFIG_WEAVE_TEST_BINARY is already set. Dev loop: run this, then
# optionally `cd web-ui && pnpm dev` for HMR on :5173.
[group('web'), doc("Spin up the web GUI at http://localhost:8765 against a folder of runbooks")]
serve dir='testdata' *ARGS: build web-build
	CONFIG_WEAVE_TEST_BINARY=${CONFIG_WEAVE_TEST_BINARY:-{{prefix}}/config-weave-linux-x86_64} \
		cargo run -p weave-server -- --dir {{dir}} \
		--config-weave target/debug/config-weave {{ARGS}}

# Assemble the runtime image from already-built artifacts (release CLI
# in dist/, server + pipeline binaries in target/release).
[private]
docker-image-assemble:
	cp target/release/weave-server dist/weave-server
	cp target/release/config-weave-pipeline dist/config-weave-pipeline
	docker build -t weave-server .

# Build the weave-server docker image. Cross-builds the static CLI (it
# doubles as the in-container test binary), builds the server + frontend
# + pipeline daemon, then assembles a slim runtime image with a static
# docker CLI.
[group('web'), doc("Build the weave-server docker image (cross CLI + server + frontend + pipeline)")]
docker-build: release server-build pipeline-build docker-image-assemble

# Run the containerized GUI: mounts the docker socket (testlab containers
# are siblings on the host daemon) and a runbooks folder. vmlab-backed
# tests and VNC are unavailable inside the container. Auth: pass
# FORGE_JWT_SECRET/FORGE_AUTH_USERS via -e, or append --no-auth.
[group('web'), doc("Run the containerized GUI with the docker socket + a runbooks folder mounted")]
docker-run dir='.' *ARGS:
	docker run --rm -p 8765:8765 \
		-v /var/run/docker.sock:/var/run/docker.sock \
		-v $(realpath {{dir}}):/runbooks \
		weave-server {{ARGS}}

# Run the pipeline daemon from the same image (overridden entrypoint):
# mounts a pipelines dir (holds pipelines.wcl) and a playbooks dir (play
# steps resolve `playbook` names under it). --no-auth for a trusted net;
# pass --forge-issuer … instead for forge-auth. `just docker-build` first.
[group('web'), doc("Run the containerized pipeline daemon with a pipelines + playbooks folder mounted")]
docker-run-pipeline pipelines='pipeline/testdata/pipelines' playbooks='testdata' *ARGS:
	docker run --rm -p 8770:8770 \
		-v $(realpath {{pipelines}}):/pipelines \
		-v $(realpath {{playbooks}}):/runbooks \
		--entrypoint config-weave-pipeline weave-server \
		--dir /pipelines --playbooks-dir /runbooks --bind 0.0.0.0 {{ARGS}}

# Start the compose monitoring test stack (docker-compose.yml): the
# weave-server image (build it first with `just docker-build` when
# stale) against testdata/, plus Prometheus (:9090) scraping /metrics
# and Loki (:3100) receiving the server + run logs — backs the
# per-service Monitoring/Logs tabs at http://localhost:8765.
[group('web'), doc("Start the compose test stack: weave-server + Prometheus (:9090) + Loki (:3100)")]
stack-up:
	docker compose up -d
	@echo
	@echo "weave-server:   http://localhost:8765"
	@echo "weave-pipeline: http://localhost:8770"
	@echo "prometheus:     http://localhost:9090"
	@echo "loki:           http://localhost:3100"

# The dev loop for the stack: rebuild the server + frontend, reassemble
# the weave-server image (reuses the cross-built CLI already in dist/ —
# run `just docker-build` instead when the CLI itself changed), then
# (re)start the stack; compose recreates containers whose image changed.
[group('web'), doc("Rebuild the weave-server image (server + frontend + pipeline, no cross build) and restart the stack")]
stack-rebuild: server-build pipeline-build docker-image-assemble stack-up

# Stop the compose monitoring test stack and remove its containers
# (named volumes with metric/log history are kept).
[group('web'), doc("Stop the compose test stack (keeps the data volumes)")]
stack-down:
	docker compose down

# Release artifacts for both PRD targets plus a checksums file.
# Requires `cross` and a container runtime; path deps are mounted into
# the build container (see Cross.toml).
[group('build'), doc("Cross-build release artifacts for both PRD targets + checksums")]
release:
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-unknown-linux-musl
	CW_WCL=$(realpath ../WCL) CW_WSCRIPT=$(realpath ../wscript) CW_FORGE=$(realpath ../forge) CARGO_TARGET_DIR=target-cross \
		cross build --release --target x86_64-pc-windows-gnu
	mkdir -p dist
	cp target-cross/x86_64-unknown-linux-musl/release/config-weave dist/config-weave-linux-x86_64
	cp target-cross/x86_64-pc-windows-gnu/release/config-weave.exe dist/config-weave-windows-x86_64.exe
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
