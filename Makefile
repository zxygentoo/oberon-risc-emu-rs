# Convenience wrapper over Cargo. Rust links all Rust/crate code statically into
# each release binary; system libraries (glibc, and the emulator's display stack)
# are resolved dynamically at run time, as usual.

CARGO ?= cargo

# Default: the windowed emulator -> target/release/risc
all:
	$(CARGO) build --release --bin risc
	@echo
	@echo "  ✓ emulator built → target/release/risc"

# Host tools -> target/release/{ob2unix,asciidecoder,norebo,build-image}
tools:
	$(CARGO) build --release -p oberon-tools
	@echo
	@echo "  ✓ tools built → target/release/"
	@echo "      ob2unix  asciidecoder  build-image"

# Whole-workspace test suite.
test:
	$(CARGO) test --workspace

clean:
	$(CARGO) clean

.PHONY: all tools test clean
