#!/usr/bin/env bash
# Offline strict-schema smoke. No provider login, credential copy or external request.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
exec /usr/bin/python3 -m pytest -q scripts/tests/test_schema_smoke_report.py
