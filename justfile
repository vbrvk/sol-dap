# sol-dap development commands

# Run quick tests (no forge needed)
test:
    cargo test

# Run ALL tests including forge-dependent ones (serial to avoid conflicts)
test-all:
    cargo test -- --include-ignored --test-threads=1

# Build debug binary
build:
    cargo build

# Build release binary
build-release:
    cargo build --release

# Build Zed extension
build-ext:
    cd zed-ext && cargo build --release --target wasm32-wasip1

# Install sol-dap to cargo bin
install:
    cargo install --path .

# Check everything compiles
check:
    cargo check
    cd zed-ext && cargo check --target wasm32-wasip1

# Run clippy
lint:
    cargo clippy -- -W warnings

# Format code
fmt:
    cargo fmt

# Run all CI checks (same as GitHub Actions and pre-commit hook)
ci:
    cargo fmt --check
    cargo clippy -- -W warnings
    cargo test

# Install pre-commit hook
install-hooks:
    cp contrib/pre-commit .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit

# Clean build artifacts
clean:
    cargo clean
    cd zed-ext && cargo clean

# Rebuild test fixtures (run after changing .sol files)
rebuild-fixtures:
    cd tests/fixtures/sample-project && forge build --force

# Manual DAP test: send initialize + launch
test-dap TEST="testIncrement" CONTRACT="CounterTest":
    @echo 'Testing DAP handshake...'
    @BODY='{"seq":1,"type":"request","command":"initialize","arguments":{"adapterID":"sol-dap"}}'; \
    LEN=$$(echo -n "$$BODY" | wc -c | tr -d ' '); \
    BODY2='{"seq":2,"type":"request","command":"launch","arguments":{"request":"launch","project_root":"'$$(pwd)/tests/fixtures/sample-project'","test":"{{TEST}}","contract":"{{CONTRACT}}"}}'; \
    LEN2=$$(echo -n "$$BODY2" | wc -c | tr -d ' '); \
    printf "Content-Length: $${LEN}\r\n\r\n$${BODY}Content-Length: $${LEN2}\r\n\r\n$${BODY2}" | \
    timeout 15 ./target/debug/sol-dap 2>/dev/null | \
    python3 -c "import json,re,sys; data=sys.stdin.buffer.read().decode(); parts=re.split(r'Content-Length: \d+\r?\n\r?\n', data); [print(json.dumps(json.loads(p), indent=2)) for p in parts if p.strip()]"
