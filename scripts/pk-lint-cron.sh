#!/usr/bin/env bash
# =============================================================================
# pk-lint-cron.sh — Scheduled Knowledge Base Audit
#
# Run this on a cron or K3s CronJob to keep the wiki clean automatically.
# Emits structured JSON suitable for Prometheus metrics or Langfuse tracing.
#
# Cron example (daily at 3am):
#   0 3 * * * /path/to/pk-lint-cron.sh >> ~/.prometheus/knowledge/lint.log 2>&1
#
# K3s CronJob: see deployment/k3s/pk-lint-cronjob.yaml
# =============================================================================

set -euo pipefail

PK_BIN="${PK_BIN:-pk}"
PK_KB_DIR="${PK_KB_DIR:-$HOME/.prometheus/knowledge}"
AUTO_FIX="${AUTO_FIX:-false}"
LOG_FORMAT="${LOG_FORMAT:-text}"  # text | json

TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

if [[ "$LOG_FORMAT" == "json" ]]; then
    echo "{\"ts\":\"$TS\",\"event\":\"lint_start\",\"kb_dir\":\"$PK_KB_DIR\"}"
else
    echo "[$TS] Starting lint pass on $PK_KB_DIR"
fi

FIX_FLAG=""
if [[ "$AUTO_FIX" == "true" ]]; then
    FIX_FLAG="--fix"
fi

OUTPUT=$("$PK_BIN" --kb-dir "$PK_KB_DIR" lint $FIX_FLAG 2>&1) || true

if [[ "$LOG_FORMAT" == "json" ]]; then
    # Escape output for JSON
    ESCAPED=$(echo "$OUTPUT" | python3 -c 'import sys,json; print(json.dumps(sys.stdin.read()))')
    echo "{\"ts\":\"$TS\",\"event\":\"lint_complete\",\"output\":$ESCAPED}"
else
    echo "$OUTPUT"
    echo "[$TS] Lint pass complete"
fi
