# The merge bar: everything a change must pass before it can land on main.
#
# Self-sufficient from a clean checkout — the cross-repo deps (wcl_lang,
# wscript, wscript-std) are GitHub git dependencies pinned by Cargo.lock, so
# no sibling checkout and no prior build step is assumed.
#
# The testlab suites (`just test-lab`, `just test-lab-vm`, `just test-ad`,
# `just test-pkgs`) are deliberately out of the gate: they need vmlab, KVM,
# cross-built binaries and in some cases a prebuilt VM template, so they
# cannot run unattended on an arbitrary machine.

import '../shared.just'

# Recipes in a module run from the module's own directory; the gate is about
# the workspace at the repo root.
set working-directory := '../..'

[default, private]
main:
	@just --list ci

# Everything a change must pass before it can merge
[group('ci')]
check: lint fmt-check test

# Lint the workspace, tests and benches included
[group('ci')]
lint: require-clippy
	# --all-targets so tests and benches are linted too — without it a lint
	# in test code only surfaces after a push.
	cargo clippy --all-targets -- -D warnings

# Check formatting, rewriting nothing
[group('ci')]
fmt-check: require-rustfmt
	cargo fmt --check

# Run the host-independent test suite (the CLI + the weave-docjson crate)
[group('ci')]
test: require-cargo
	# weave-docjson is named explicitly — the default-members setting would
	# otherwise skip it.
	cargo test
	cargo test -p weave-docjson
