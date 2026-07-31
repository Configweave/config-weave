# Helpers shared by the modules under .just/.
#
# The require-* guards exist so a missing tool reports as a missing tool
# rather than as a confusing build error — a caller can then tell "the code
# is wrong" apart from "this machine can't run the check".

[private]
require-cargo:
	@command -v cargo > /dev/null 2>&1 || (echo "cargo is required — install a Rust toolchain (https://rustup.rs)" && exit 1)

[private]
require-clippy: require-cargo
	@cargo clippy --version > /dev/null 2>&1 || (echo "the clippy component is required — rustup component add clippy" && exit 1)

[private]
require-rustfmt: require-cargo
	@cargo fmt --version > /dev/null 2>&1 || (echo "the rustfmt component is required — rustup component add rustfmt" && exit 1)
