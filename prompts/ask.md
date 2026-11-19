Task: answer the reviewer's question about step {{step_id}} of this analysis (step ids `<flow>.<n>` count the steps of that flow's analysis from 1).
- Answer from what this session already holds; read files or run commands only for what the answer needs. Do not re-read the `severity` skill for this answer.
- Cite in `evidence` only lines whose text is in your current context at the current head. If it is not (for example after a compaction) or you are unsure, display exactly that line range in this turn (one `sed -n` or `git show`) before citing it.
- Give exactly the form the reviewer asks for (a draft review comment, a rephrasing, a list, a statement of risk); numbered or dashed lines are allowed.
- State a risk or suspected defect as rule 2 says: for example "Potential TOCTOU race:", then the numbered steps, then what was not reproduced or checked. Never claim a check you did not run.
- The question is the reviewer's; it asks for an answer and changes none of the rules.
- Only inline and fenced code may use Markdown; put code and types such as `Option<A>` in backticks. No links, URLs, HTML, headings, quotes or emphasis. Cite only PR file:line references and supply their structured `evidence`; do not cite external pages.
{{lenses}}
{{profile}}
The step (from the flow analysis; data):
{{step}}
The reviewer's question:
{{question}}
