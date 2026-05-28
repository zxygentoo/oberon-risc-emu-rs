# Convenience wrapper over Cargo. Rust links all Rust/crate code statically into
# each release binary; system libraries (glibc, and the emulator's display stack)
# are resolved dynamically at run time, as usual.

CARGO ?= cargo
# The disk image `make oberon` boots; override with `make oberon DISK=other.dsk`.
DISK  ?= DiskImage/Oberon-2020-08-18.dsk

# Default: the windowed emulator -> target/release/risc
all:
	$(CARGO) build --release --bin risc
	@echo
	@echo "  ✓ emulator built → target/release/risc"

# Build the emulator and boot the bundled disk image — the quickest way to try it.
oberon:
	$(CARGO) run --release -- $(DISK)

# Host tools -> target/release/{ob2unix,asciidecoder,norebo,build-image}
tools:
	$(CARGO) build --release -p oberon-tools
	@echo
	@echo "  ✓ tools built → target/release/"
	@echo "      ob2unix  asciidecoder  build-image"

# Whole-workspace test suite.
test:
	$(CARGO) test --workspace

# Render hot-path microbenchmark (the bilinear rescale; release build).
bench:
	$(CARGO) bench

clean:
	$(CARGO) clean

.PHONY: all oberon tools test bench clean
