#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${HCOM_MOCK_TOOLS_PREFIX:-$ROOT/target/mock-tools}"
CACHE="${HCOM_MOCK_TOOLS_NPM_CACHE:-$ROOT/target/npm-cache}"

mkdir -p "$PREFIX" "$CACHE"

if [[ "$#" -gt 0 ]]; then
  packages=("$@")
else
  packages=(
    "@openai/codex@0.139.0"
    "@anthropic-ai/claude-code@2.1.177"
  )
fi

claude_version=""
has_claude_native=0
for package in "${packages[@]}"; do
  case "$package" in
    @anthropic-ai/claude-code@*)
      claude_version="${package##*@}"
      ;;
    @anthropic-ai/claude-code)
      claude_version="2.1.177"
      ;;
    @anthropic-ai/claude-code-*)
      has_claude_native=1
      ;;
  esac
done

if [[ -n "$claude_version" && "$has_claude_native" -eq 0 ]]; then
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64) claude_platform="darwin-arm64" ;;
    Darwin:x86_64) claude_platform="darwin-x64" ;;
    Linux:x86_64) claude_platform="linux-x64" ;;
    Linux:aarch64 | Linux:arm64) claude_platform="linux-arm64" ;;
    *)
      printf 'Unsupported Claude mock-test platform: %s %s\n' "$os" "$arch" >&2
      exit 1
      ;;
  esac
  packages+=("@anthropic-ai/claude-code-$claude_platform@$claude_version")
fi

npm install \
  --global \
  --prefix "$PREFIX" \
  --cache "$CACHE" \
  --no-audit \
  --no-fund \
  --fetch-retries 5 \
  --fetch-retry-mintimeout 20000 \
  --fetch-retry-maxtimeout 120000 \
  --fetch-timeout 600000 \
  "${packages[@]}"

if [[ -n "$claude_version" ]]; then
  node "$PREFIX/lib/node_modules/@anthropic-ai/claude-code/install.cjs"
fi

printf '%s\n' "$PREFIX/bin"
