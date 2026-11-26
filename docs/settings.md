# Settings guide

<!-- Generated from web/src/components/settings/settingsHelp.ts; the in-app guide at /settings/help shows
the same text. Update: UPDATE_GENERATED=1 npx vitest run src/components/settings/settingsHelp.test.ts -->

The guide follows the Settings pages. Find what each setting does, when diffpilot uses it, and what happens when it is empty. The `?` next to a setting opens its entry.

## Account

### Sign out

- **What:** Ends your session in this browser. Other browsers stay signed in.
- **When used:** When you press Sign out.
- **Empty or default:** Your unsent comment drafts in this tab are removed. Your GitHub pending review stays on GitHub.
- **Example:** Sign out on a shared computer before you leave it.

With the nightly scan on and a time zone stored, the server keeps your GitHub sign-in after your last session ends, so the nightly scan can keep running. It keeps it for up to 30 days after your last sign-in; after that the nightly scan says it needs you to sign in again.

Personal access tokens keep working after Sign out.

### Sign out everywhere

- **What:** Ends every session of yours in every browser, revokes all your personal access tokens and deletes the GitHub sign-in this server keeps for the nightly scan. Nothing changes on GitHub.
- **When used:** When you press Sign out everywhere and confirm. The API does the same for POST /auth/logout with everywhere set.
- **Empty or default:** Until you sign in again, the nightly scan records that it needs you to sign in. Your unsent comment drafts in this tab are removed.
- **Example:** Use it when a token or a browser session may have leaked.

### Access tokens

- **What:** Personal access tokens let agents use diffpilot over HTTP. A read token is read-only. Add review_loop_execute for an agent that claims decisions and reports their effects.
- **When used:** When an agent sends Authorization: Bearer srp_… with a request.
- **Empty or default:** No tokens: agents cannot read your data.
- **Limits:** Name of 1 to 60 characters; lifetime of 30, 90 (default) or 365 days; at most 10 live tokens. The secret is shown once.
- **Example:** A token named laptop-agent for 90 days. The agent API: docs/contracts/agent-api.md.

Revoking a token stops it at once. Sign out everywhere revokes every token.

## LLM & usage

### Model

- **What:** The LLM model of every job on this installation: analyses, Ask answers and the ranking of My work. One choice for everyone.
- **When used:** Each new job reads the current choice; a running job keeps the one it started with.
- **Empty or default:** Sol 6.1 until someone saves another model.
- **Limits:** One of the listed models.
- **Example:** Pick Astra 6 for a hard pull request, then switch back.

### Reasoning effort

- **What:** How much the model reasons in analysis jobs: the plan, the PR context and each flow.
- **When used:** Each new analysis job reads it.
- **Empty or default:** High until someone saves another effort.
- **Limits:** Low, medium or high. Higher effort can take longer.
- **Example:** Medium is often enough for small pull requests.

### Ask effort

- **What:** How much the model reasons when it answers a question you ask about a step.
- **When used:** Each new Ask answer reads it. It applies to everyone on this installation.
- **Empty or default:** Medium until someone saves another effort.
- **Limits:** Low, medium or high. Lower effort answers sooner.
- **Example:** Low for quick questions such as where a value is set.

### Dashboard ranking effort

- **What:** How much the model reasons when it ranks the items of My work.
- **When used:** Each ranking job of a refresh reads it. It applies to everyone on this installation.
- **Empty or default:** Low until someone saves another effort.
- **Limits:** Low, medium or high.
- **Example:** Medium if the ranking reasons look too thin.

### LLM status

- **What:** The outcome of the most recent LLM call and its time, and the tokens the provider reported for your use.
- **When used:** Updated by every LLM call. Read-only.
- **Empty or default:** Not known yet until the first analysis calls the LLM.
- **Example:** After a failed call the provider's message stands below the status.

### Token usage

**Today**, **Last 7 days** and **This month** use the server container's time zone. The deployment sets no `TZ`,
so the windows use UTC. Today starts at 00:00 UTC, Last 7 days starts at 00:00 UTC six days ago, and This month
counts usage from 00:00 UTC on the first day of the current calendar month.

### Language of LLM texts

- **What:** The language of explanations, questions and assessments the LLM writes for you.
- **When used:** Read when an analysis starts; an analysis keeps the language it started with, its Ask answers too. Dashboard ranking texts follow it from the next refresh; until they are judged again they show as provisional.
- **Empty or default:** English.
- **Limits:** English, Polski or Русский.
- **Example:** Polski, then analyse the pull request again to get its texts in Polish.

### Ranking limits

- **What:** The LLM judges at most 48 items per refresh and 120 per day for each user.
- **When used:** Every ranking. Items not judged again because they did not change cost nothing.
- **Empty or default:** Fixed limits; there is no setting.
- **Example:** A first refresh with 150 items judges 48; the rest stay provisional until later refreshes.

## GitHub sync

### GitHub App

- **What:** The installations of the diffpilot GitHub App your GitHub sign-in can see. Comments go into your pending review only where the App is installed.
- **When used:** Read when Settings opens. Read-only; install or change the App on GitHub.
- **Empty or default:** No installation: pull requests can be analysed, but comments must be written on GitHub.
- **Example:** Install the App on dashpay to comment on dashpay/platform from here.

The card shows the URL to enter in the GitHub App when the operator configures it. My work receives live GitHub
updates for tracked repositories with an active App installation and a configured webhook secret. Your tenant
tracks repositories through its repository rules, owners listed under **When GitHub search refuses an account**,
active watches, analyses and My work snapshots. Use **Refresh** to recover missed facts or wait for the nightly
scan. Existing judgements can become outdated when their facts change.

### Controlled organisations

- **What:** GitHub organisations or users whose repositories are yours. The Dependencies view marks their repositories and crates "yours", lists their crates among a changed crate's dependencies, and the PR-context agent looks up issues and pull requests in them.
- **When used:** Every time a Dependencies view or a PR context prompt is built; a change applies at once, without analysing again.
- **Empty or default:** Only the pull request's own owner is controlled; it always is.
- **Limits:** Up to 50 GitHub logins (letters, digits and -, up to 39 characters), one list for the whole installation.
- **Example:** dashpay: a pull request in dashpay/dash-evo-tool shows dashpay/platform crates as yours.

### Linked logins

- **What:** Other GitHub accounts of yours. Their pull requests, review requests, assigned and mentioning issues count as yours, and their approvals and ownership count as yours. A linked account can use the maintainer-decision v1 marker to leave a proposal for you to confirm.
- **When used:** Every refresh searches them together with your own login.
- **Empty or default:** Only your own login is scanned; it is always scanned.
- **Limits:** Up to 5 GitHub logins (letters, digits, - or _, up to 39); not your own login.
- **Example:** A bot account that opens pull requests you take care of (e.g. Claudius-Maginificent): put it here so its pull requests count as yours, and keep it in Known bots so its reviews count as a bot's. Do not put it into Excluded authors.

A linked account can relay a decision proposal. In the item's thread, the account posts the decision in plain
text and puts the matching marker on a line of its own outside quotes and code. For example, a proposal to reject
uses `<!-- maintainer-decision v1 kind=reject -->`. Diffpilot checks the account's numeric GitHub ID and shows an
eligible proposal in **Needs you** for you to confirm.

### When GitHub search refuses an account

- **What:** Optional. Organisations or users whose open pull requests and issues are listed directly when GitHub search will not answer for one of your accounts. GitHub search refuses some logins, typically bot or restricted accounts.
- **When used:** Only in a refresh where search refuses one of your logins. The scan then lists the open pull requests and issues of every repository of these owners and keeps those involving the refused login, and the charts count the refused login's pull requests in these owners' repositories and in your repository rows, whoever merged them. Its reviews and its pull requests elsewhere come from its last 300 public events only.
- **Empty or default:** Nothing is listed; the refused account's work is missing from My work.
- **Limits:** Up to 5 owners. Each costs extra GitHub requests per refresh, so keep the list short.
- **Example:** dashpay.

How you find out: after a refresh, What the scan missed names the accounts search refused and how far back their history is complete.

### Look-back days

- **What:** How many days an issue counts as new.
- **When used:** Every refresh: an issue opened at most this many days before the scan shows the tag new, with its opening date in the tip. It does not change the ranking.
- **Empty or default:** 7 days.
- **Limits:** A whole number from 1 to 30.
- **Example:** 14 if you look at My work once a week.

### Ready for a human label

- **What:** The label that marks a pull request ready for a human review.
- **When used:** Every ranking: a pull request with this label ranks higher. One of yours with it needs a human: it is never listed under Ready to merge, and while nothing is your move (no conflicts, CI green with ignored checks left out, no change request, no unresolved thread, no comment awaiting your reply, no review asked of you) it is listed under Waiting on others only, not in Needs you, even when it is approved or you could merge it.
- **Empty or default:** ready-for-human.
- **Limits:** 1 to 50 characters; compared without regard to case.
- **Example:** needs-review.

### Excluded authors

- **What:** Banned authors. Pull requests and issues written by these accounts are never shown in Needs you — also when a review is requested from you or they are assigned to you. Being listed does not make an account a bot.
- **When used:** Only on My work (the scan and the ranking); never in pull request analyses, flows or Ask.
- **Empty or default:** dependabot, dependabot[bot].
- **Limits:** Up to 20 GitHub logins, optionally ending in [bot].
- **Example:** Add renovate[bot] to hide Renovate's pull requests.

### Known bots

- **What:** Accounts treated as bots: their review requests and comments do not count as a person waiting for you, they are not listed among reviewers you wait on, and their reviews count as bot reviews. GitHub Apps (…[bot]) are always bots and need not be listed. Nothing else is built in.
- **When used:** Only on My work (the scan and the ranking); never in pull request analyses, flows or Ask.
- **Empty or default:** thepastaclaw, Claudius-Maginificent and DCG-Claude. An empty list leaves only GitHub Apps as bots.
- **Limits:** Up to 20 GitHub logins, optionally ending in [bot]. Your own and linked logins may be listed.
- **Example:** Add a review bot that runs under a normal user account, so its review requests do not wait on you.

### Show my MemCan TODOs

- **What:** Lists your open MemCan TODOs on My work next to pull requests and issues.
- **When used:** Every refresh reads them from the MemCan projects below. Switching it off hides them at once.
- **Empty or default:** On. Without MemCan on the server there are no TODOs and the switch is off.
- **Example:** Off if you keep your TODOs elsewhere.

When MemCan cannot be reached, the TODO rows of the earlier scan stay and each says not refreshed.

### Extra MemCan projects

- **What:** MemCan projects whose TODOs are read besides the projects named after your repository rows.
- **When used:** Every refresh with TODOs on.
- **Empty or default:** Only the repository rows' names are read (dashpay/platform gives platform).
- **Limits:** Up to 10 names (letters, digits, . _ or -, up to 100).
- **Example:** home-infra.

### Run nightly scan

- **What:** Runs once a night and judges only items never judged. Updates in between come from GitHub events and Refresh.
- **When used:** At the local time below, with your GitHub sign-in.
- **Empty or default:** On. It needs a time zone; without one it does not run.
- **Example:** On, 06:00: My work is ready when you start.

While it is on, the server keeps your GitHub sign-in for up to 30 days after your last sign-in. When the sign-in has ended, My work says the nightly scan needs you to sign in again.

### Time of day

- **What:** The local time of the nightly scan.
- **When used:** Every day in your time zone.
- **Empty or default:** 06:00.
- **Limits:** HH:MM in steps of 15 minutes.
- **Example:** 07:30.

### Time zone

- **What:** Your time zone for the nightly scan, the day of the daily ranking limit and the history charts.
- **When used:** Your first visit to My work stores your browser's zone when none is stored.
- **Empty or default:** Not set yet: the nightly scan does not run.
- **Limits:** A time zone name such as Europe/Warsaw.
- **Example:** Europe/Warsaw.

### Slack names

- **What:** Your table of GitHub logins and the name to write for each in Slack. Copy for Slack in Waiting on others (the Slack icon of a row, and Copy for Slack next to Copy list) writes the Slack name instead of @login; Copy list and the plain copy icon keep @login.
- **When used:** Only when you copy for Slack on My work. Nothing is sent to Slack: the text goes to your clipboard.
- **Empty or default:** No rows: Copy for Slack writes @login for every login, like Copy list.
- **Limits:** Up to 100 rows. Each GitHub login once (any case); the Slack name is copied as you type it, 1 to 80 characters without control characters.
- **Example:** jun-park → @jun.park.

## Repositories

### Repository settings

- **What:** Read-only facts about one repository: the built-in rules that put tests, docs and generated files under Not analysed, the related repositories and where the project profile comes from.
- **When used:** Shown for repositories that have an analysed pull request.
- **Empty or default:** Analysis rules appear after the first analysis of that repository.
- **Example:** dashpay/platform: related repositories found in Cargo.toml and Cargo.lock.

### Repository

- **What:** A repository you work in, as owner/name. The row holds that repository's short name, issue watch, ownership policy and release branches; the repository name is also read as a MemCan project for TODOs.
- **When used:** Every refresh. Your pull requests and issues are found by search even without a row.
- **Empty or default:** No rows: only search results, without short names, watched issues or policies.
- **Limits:** Up to 50 rows, each repository once.
- **Example:** dashpay/platform.

### Short name

- **What:** The prefix of the repository's item ids on My work and of the tab title of its reviews.
- **When used:** Whenever an item or a review of the repository is shown.
- **Empty or default:** The repository name: platform#1187 instead of PLAT-1187.
- **Limits:** Capital letters, digits and -, starting with a letter or digit, up to 12; unique among the rows.
- **Example:** DET for dashpay/dash-evo-tool: items show as DET-512, the review tab as DET#512.

### Watch issues

- **What:** Also lists every open issue of the repository, not only the issues assigned to you or mentioning you.
- **When used:** Every refresh searches the open issues of the watched repositories.
- **Empty or default:** Off: only issues assigned to you or mentioning you.
- **Example:** On for a small repository where you triage all issues.

### Ownership policy

- **What:** Optional. A JSON file listing the paths you own in this repository, in the format of dashpay/stale_prs_are_bad. In a repository with a policy, a pull request of others stays in Needs you only when it changes a fair amount of code in your paths; pull requests directed at you always stay.
- **When used:** Every refresh reads the file for the repository's pull requests.
- **Empty or default:** Leave both fields empty when the repository has no such file: every open pull request counts.
- **Limits:** Both fields or neither. The repository as owner/name; the path relative, ending with .json, without .., % or \\.
- **Example:** Repository dashpay/stale_prs_are_bad, path policies/platform.json.

Format: the JSON files of https://github.com/dashpay/stale_prs_are_bad (folder policies/). The tool reads only areas: a list of objects with paths, owners and reviewers. The paths of every area that names you or a linked login as owner or reviewer are yours; a path matches every file that starts with it.

What counts: a pull request whose changes in your paths are only comments or docs, or only a few code lines (fewer than 12 distinct code lines, or fewer than 8 in any one file), is left out of Needs you, unless it is directed at you: your own (or a linked login's), assigned to you, or with your review requested directly. A review requested only from one of your teams does not count as directed. Excluded authors are left out whatever the policy says.

When the file cannot be read: pull requests directed at you stay as usual; those of others not directed at you stay in the list with ownership unverified, the LLM is asked whether they concern you and may leave them out (Excluded: judged outside your areas by the LLM), and What the scan missed names the file.

Worked examples: dashpay/platform uses dashpay/stale_prs_are_bad with policies/platform.json; dashpay/tenderdash uses policies/tenderdash.json; dashpay/dash-evo-tool uses policies/dash-evo-tool.json. A repository without a policy file: leave both fields empty.

### Release branches

- **What:** Names of the repository's release and long-lived branches. A pull request based on such a branch is not treated as stacked on another pull request, one that targets such a branch (other than the default branch) counts as targeting a release line, and Ready to merge lists your pull requests into such a branch as into the default branch.
- **When used:** Every ranking of the repository's pull requests, and Ready to merge.
- **Empty or default:** Leave empty for the built-ins: main, master, release/\*, hotfix/\*, vX.Y, vX.Y-dev. Your list replaces the built-ins; it does not add to them.
- **Limits:** Up to 10 patterns, separated by spaces or commas; letters, digits, . _ / - and \*. X.Y stands for a version of two or more numbers (1.8, 5.1, 2.0.1); \* matches any text.
- **Example:** vX.Y-dev matches v1.8-dev and v5.1-dev; release/\* matches release/2.1; v\*-dev works too.

### Ignored checks

- **What:** Names of the repository's checks and commit statuses that are gates rather than build, test or lint results (approval, policy and review-bot checks). They never make CI failing or running; a check of them that has not passed is named in the tip of CI passing.
- **When used:** Every refresh of the repository's pull requests: CI passing, Ready to merge (a PR GitHub calls unstable counts as clean when only ignored, skipped or neutral checks have not passed) and Waiting on others (CI green).
- **Empty or default:** Leave empty for the built-ins: \*hygiene\*, PR title, \*semantic-pull-request\*, \*milestone\*, \*CodeRabbit\*, review, policy\*, PastaClaw review. Your list replaces the built-ins; it does not add to them. Among the built-ins, policy\* stands for policy alone or followed by a space or sign (policy / reconcile).
- **Limits:** Up to 20 names, separated by commas; each 1 to 100 characters. A name matches the whole check name without regard to case; \* matches any text. The check's own name counts, without its workflow.
- **Example:** \*hygiene\*, PR title, PastaClaw review, docs-preview

## Review loop

### Browser notifications

- **What:** A browser notification when an Ask answer of yours arrives, a flow is analysed, or an analysis is ready to review, finishes or fails. It names the pull request and the flow or step, never the answer text; a click opens it.
- **When used:** While a diffpilot tab showing that review or its progress is open and you are in another tab or window. A tab you are looking at does not notify. Several open tabs notify once.
- **Empty or default:** Off. The browser asks for permission the first time you enable notifications. Each browser keeps its own choice.
- **Limits:** Needs https (or localhost) and the browser's permission. Closed tabs do not notify. Mobile browsers show none.
- **Example:** Ask a question, switch to your editor, and get a notification when the answer is ready.

## MemCan

### Memory

- **What:** Whether MemCan, the memory service, is configured and reachable. Analyses read project knowledge from it and My work reads TODOs from it.
- **When used:** Shown as it was last checked. Read-only.
- **Empty or default:** Without MemCan, analyses run without it and My work shows no TODOs.
- **Example:** Not configured: the server has no MemCan address.
