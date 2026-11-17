Task: describe this pull request at project scale, so the reviewer can judge whether it is the right change for the project, not only whether its lines are correct.
- `problem`, `why_needed`, `what_it_gives`: from the code and the pull request text; say when something is only claimed by the text.
- `related_work`: with the GitHub tools, search this repository and the related repositories for duplicate or competing pull requests, stacked pull requests, linked issues and companion changes; each item with `reference` (`owner/repo#n` or URL), `kind` and one line on the overlap. Without GitHub tools: [].
- `fit_questions`: questions about the fit at project scale, never verdicts, each ending with "?", with the evidence lines that motivate it: right layer or repository? oversized for the problem? duplicates an existing mechanism? Ask only what the reviewer cannot settle from the diff alone.
{{lenses}}
{{profile}}
{{pr_text}}
{{related_repos}}
{{dependency_files}}
