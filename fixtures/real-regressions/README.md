# Real flow regression fixtures

Public outputs from dashpay/dash-evo-tool#1169 (`det-1169-flow.json`) and dashpay/tenderdash#1578
(`td-1578-flow.json`), recorded on 2026-11-17. Only the flow documents are retained; plan and PR context are
irrelevant to the missing-assessment regression. Flow contents are unchanged, including every null assessment.

The verifier test provides in-bounds line facts and an empty diff to isolate assessment validation. It must
reject every missing assessment. These are negative fixtures, not valid examples for the agent.
