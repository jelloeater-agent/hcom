set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

mock-bin := justfile_directory() + "/target/mock-tools/bin"

mock-tools:
    ./scripts/install-mock-tools.sh

ci: mock-tools
    cargo fmt --all -- --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
    PATH="{{mock-bin}}:$PATH" cargo test --locked --test real_tool_codex -- --ignored --nocapture --test-threads=1
    PATH="{{mock-bin}}:$PATH" cargo test --locked --test real_tool_claude -- --ignored --nocapture --test-threads=1
    PATH="{{mock-bin}}:$PATH" cargo test --locked --test test_relay_roundtrip -- --ignored --nocapture --test-threads=1
