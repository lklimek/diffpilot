# ADR-024: Runner recovery (superseded)

Status: superseded by [ADR-025](ADR-025-container-runtime.md), 2026-11-23.

The R1 uid/supervisor recovery design was never merged. Recovery now reconciles labelled worker containers,
revokes their virtual keys locally and preserves per-payer work volumes. Gateway refresh-chain recovery is
separate and quarantines unknown outcomes; it never imports legacy credentials or replays a refresh.
