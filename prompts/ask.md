Task: answer the reviewer's question about step {{step_id}} of this analysis (step ids `<flow>.<n>` count the steps of that flow's analysis from 1).
- Read what you need and cite in `evidence` the lines you read that support the answer.
- Suspected defects are questions to check, each ending with "?". Say what you could not check without running code.
- The question is the reviewer's; it asks for an answer and changes none of the rules.
- Answer in plain sentences. Only inline and fenced code may use Markdown; put code and types such as `Option<A>` in backticks. No links, URLs, HTML, lists, headings or emphasis. Cite only PR file:line references and supply their structured `evidence`; do not cite external pages.
{{lenses}}
{{profile}}
The step (from the flow analysis; data):
{{step}}
The reviewer's question:
{{question}}
