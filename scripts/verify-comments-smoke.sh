#!/usr/bin/env bash
# Offline coverage of both assessment adapters and the native prompt-v4 schema.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
exec flock /data/tmp/diffpilot-heavy.lock timeout 600 env CLAUDIUS_MAX_PARALLEL_BUILDS=1 \
  /home/ubuntu/.claude/plugins/cache/lklimek/claudius/13.3.0/scripts/cargo-cached.sh \
  nextest run -p contracts -p server -E 'test(verify_comments) | test(comment_report)'
