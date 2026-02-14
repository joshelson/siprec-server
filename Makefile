.PHONY: all build test check fmt clippy doc clean coverage install-hooks setup help

# Default target
all: fmt clippy test doc

#------------------------------------------------------------------------------
# Development Setup
#------------------------------------------------------------------------------

# Full development setup (run this first!)
setup: check-tools install-hooks
	@echo ""
	@echo "Development environment ready!"
	@echo ""
	@echo "Quick commands:"
	@echo "  make quick    - Format + lint (fast)"
	@echo "  make test     - Run all tests"
	@echo "  make ci       - Full CI check"
	@echo "  make coverage - Generate coverage report"
	@echo ""

# Check required tools are installed
check-tools:
	@echo "Checking required tools..."
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust: https://rustup.rs"; exit 1; }
	@echo "  cargo"
	@command -v rustfmt >/dev/null 2>&1 || { echo "rustfmt not found. Run: rustup component add rustfmt"; exit 1; }
	@echo "  rustfmt"
	@command -v cargo-clippy >/dev/null 2>&1 || { echo "clippy not found. Run: rustup component add clippy"; exit 1; }
	@echo "  clippy"
	@if command -v cargo-llvm-cov >/dev/null 2>&1; then \
		echo "  cargo-llvm-cov (coverage)"; \
	else \
		echo "  cargo-llvm-cov not found (optional). Install: cargo install cargo-llvm-cov"; \
	fi
	@echo ""

# Install git pre-commit hook
install-hooks:
	@if [ -d .git ]; then \
		mkdir -p .git/hooks; \
		cp scripts/pre-commit .git/hooks/pre-commit; \
		chmod +x .git/hooks/pre-commit; \
		echo "Pre-commit hook installed"; \
	else \
		echo "Not a git repository, skipping hook installation"; \
	fi

# Show available commands
help:
	@echo "SIPREC Server - Development Commands"
	@echo ""
	@echo "Setup:"
	@echo "  make setup        - First-time development setup"
	@echo "  make install-hooks - Install git pre-commit hooks"
	@echo "  make check-tools  - Verify required tools are installed"
	@echo ""
	@echo "Development:"
	@echo "  make quick        - Format + clippy (fast feedback)"
	@echo "  make test         - Run all tests"
	@echo "  make test-unit    - Run unit tests only (faster)"
	@echo "  make ci           - Run full CI checks locally"
	@echo ""
	@echo "Build:"
	@echo "  make build        - Debug build"
	@echo "  make release      - Release build"
	@echo "  make doc          - Build and open documentation"
	@echo ""
	@echo "Quality:"
	@echo "  make fmt          - Format code"
	@echo "  make clippy       - Run linter"
	@echo "  make coverage     - Generate coverage report"
	@echo ""
	@echo "Maintenance:"
	@echo "  make clean        - Remove build artifacts"
	@echo ""

#------------------------------------------------------------------------------
# Build
#------------------------------------------------------------------------------

# Build the project
build:
	cargo build

# Build release
release:
	cargo build --release

#------------------------------------------------------------------------------
# Testing
#------------------------------------------------------------------------------

# Run all tests
test:
	cargo test --all-features

# Run only unit tests (fast)
test-unit:
	cargo test --lib --bins

# Check compilation
check:
	cargo check --all-targets --all-features

#------------------------------------------------------------------------------
# Code Quality
#------------------------------------------------------------------------------

# Format code
fmt:
	cargo fmt --all

# Check formatting
fmt-check:
	cargo fmt --all -- --check

# Run clippy lints
clippy:
	cargo clippy --all-targets --all-features -- -D warnings

# Build documentation
doc:
	cargo doc --no-deps --all-features --open

# Run coverage report
coverage:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { echo "cargo-llvm-cov not found. Install: cargo install cargo-llvm-cov"; exit 1; }
	cargo llvm-cov --all-features --html --open
	@echo "Coverage report: target/llvm-cov/html/index.html"

# Run all CI checks locally
ci: fmt-check clippy test
	@cargo doc --no-deps --all-features 2>/dev/null
	@echo ""
	@echo "All CI checks passed!"

# Quick check (format + clippy only)
quick:
	cargo fmt --all
	cargo clippy --all-targets --all-features -- -D warnings

#------------------------------------------------------------------------------
# Maintenance
#------------------------------------------------------------------------------

# Clean build artifacts
clean:
	cargo clean
