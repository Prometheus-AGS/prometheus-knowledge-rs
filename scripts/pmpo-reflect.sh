#!/usr/bin/env bash
# =============================================================================
# pmpo-reflect.sh — PMPO Reflect Stage Hook
#
# Ingests a session summary into prometheus-knowledge after every Claude Code
# or UAR agent session. Wires the PMPO Reflect stage to the Librarian's
# compile() operation, making every session a durable knowledge event.
#
# Usage:
#   pmpo-reflect.sh [summary_file] [--source LABEL]
#
#   If no file is given, reads from STDIN.
#
# Integration:
#   Claude Code → Settings → Hooks → PostSession:
#     /path/to/pmpo-reflect.sh "$CLAUDE_SESSION_SUMMARY" --source "claude-code:$CLAUDE_SESSION_ID"
#
#   UAR post-execution (add to your UAR's reflect handler):
#     echo "$REFLECT_CONTENT" | pmpo-reflect.sh --source "uar:$SESSION_ID"
#
# Environment:
#   PK_BIN         Path to `pk` binary (default: pk on PATH)
#   PK_KB_DIR      Knowledge base dir (default: ~/.prometheus/knowledge)
# =============================================================================

set -euo pipefail

PK_BIN="${PK_BIN:-pk}"
PK_KB_DIR="${PK_KB_DIR:-$HOME/.prometheus/knowledge}"

# Parse args
SUMMARY_FILE=""
SOURCE_LABEL="pmpo:reflect:$(date +%Y-%m-%dT%H:%M:%S)"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --source)
            SOURCE_LABEL="$2"
            shift 2
            ;;
        --source=*)
            SOURCE_LABEL="${1#--source=}"
            shift
            ;;
        -*)
            echo "Unknown flag: $1" >&2
            exit 1
            ;;
        *)
            SUMMARY_FILE="$1"
            shift
            ;;
    esac
done

# Check pk binary is available
if ! command -v "$PK_BIN" &>/dev/null; then
    echo "⚠  pk binary not found at '${PK_BIN}'. Build with: cargo build -p pk-cli --release" >&2
    exit 1
fi

# Ingest
if [[ -n "$SUMMARY_FILE" && -f "$SUMMARY_FILE" ]]; then
    "$PK_BIN" --kb-dir "$PK_KB_DIR" ingest "$SUMMARY_FILE" --source "$SOURCE_LABEL"
else
    # Read from stdin
    "$PK_BIN" --kb-dir "$PK_KB_DIR" ingest --source "$SOURCE_LABEL"
fi

echo "✓ session ingested → source: $SOURCE_LABEL"
