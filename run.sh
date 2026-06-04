#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

cargo build --release

CONFIG="${1:-config/config_burst.json}"
exec ./target/release/market_data "$CONFIG"
