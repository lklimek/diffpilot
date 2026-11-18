REVIEW SCOPE: production changes only. Do not plan, title, walk through, annotate or assess test code, fixtures, mocks, benches or test helpers. Never create a flow whose subject is tests. Test code may be read only as evidence for a production step and mentioned in one clause there.
Rust comment/import changes remain available as context. Do not explain, annotate, assess or build steps/flows around comment-only or import-only changes. Their deterministic rule covers them; no flow needs to claim them. A comment contradicting code may be cited on the production-code step.

Task: partition the changed hunks listed below into flows.
- A flow is one logical change following control flow (trigger, calls, state changes, result) across files.
- Every production changed line of every listed hunk belongs to exactly one flow or to `unassigned`. Select a hunk whole (`head_start` and `head_end` null) or, when its parts belong to different flows, by head-line sub-ranges. A hunk with head length 0 (pure deletion) can only be selected whole. Use `unassigned` only for lines you cannot place, with the reason.
- Hunks that are not listed (path or line rules, identical moves, generated files) are Not analysed by the server: never assign them.
- `classification`: `behavioural`, `api_contract`, `new_code`, `refactor_pure`, `move`, `rename`, `docs_comments`, `generated_mechanical`, `config_build`, `dependency` or `other`. Use `refactor_pure` or `rename` only when you read that behaviour is unchanged; the reviewer sees it labelled as your claim.
- Size: split a flow larger than about 25 steps or 500 changed lines along control flow. Do not scatter one logical change over tiny flows: a visibility or signature change belongs to the flow that needs it.
- `importance_rank` (1 = read first, unique) orders flows by expected risk at system scale: what a production user or operator would notice if this change introduces, worsens, exposes or leaves a relevant defect; never rank the severity of a pre-existing problem that it fixes. Judge with the `severity` skill against the project profile. The flow you expect to get the highest risk band comes first. Among flows of equal expected risk: `behavioural` and `api_contract`, then `new_code`, `config_build`, `dependency` and `other`, then `move`, `rename` and `refactor_pure`, then `docs_comments`.
- `intent`: imperative, naming the entry points. `depends_on`: flows the reviewer should read first. Flow ids `f1`, `f2`, ….
{{batch}}
{{profile}}
{{moves}}
Hunks to partition: `git diff -U0 -M` from base to head, grouped by file; per hunk `<id> base <start>,<length> head <start>,<length>` (length 0 = no lines on that side, inserted after line <start>).
{{hunks}}
