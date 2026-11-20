Task: describe this pull request at project scale, so the reviewer can judge whether it is the right change for the project, not only whether its lines are correct.
- `problem`, `why_needed`, `what_it_gives`: from the code and the pull request text; say when something is only claimed by the text. Describe the severity of the pre-existing problem being fixed here; it is not the risk introduced by the change.
- `related_work`: with the GitHub tools, search this repository and the related repositories for duplicate or competing pull requests, linked issues and companion changes; each item with `reference` (`owner/repo#n` or URL), `kind` and one line on the overlap. Without GitHub tools: [].
- `stacked_pr` is a deterministic relationship: this PR's base is another OPEN PR's head, with matching repository and ref. Overlap, ancestry, a linked PR, an anticipated conflict or a PR already merged into the base does not establish it. The server currently supplies no confirmed stacked relationship; never emit `stacked_pr`. Include an item only when another kind accurately describes it, otherwise omit it. Never infer the label yourself.
- `fit_questions`: questions about the fit at project scale, never verdicts, each ending with "?", with the evidence lines that motivate it: right layer or repository? oversized for the problem? duplicates an existing mechanism? Ask only what the reviewer cannot settle from the diff alone.
{{lenses}}
{{profile}}
{{pr_text}}
{{related_repos}}
{{dependency_files}}
{{dependencies}}
