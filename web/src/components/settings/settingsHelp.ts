// Help of every Settings option: the `?` tips, the in-app guide at /settings/help and docs/settings.md (generated
// from this map by `settingsHelpMarkdown`; a test keeps the file current). Facts follow the server code.
import type {
  CreateAccessToken,
  DashboardSettings,
  LlmSelection,
  RepositoryRule,
  Settings,
} from "../../api/generated/types";

/** Sections in the order of the Settings page. */
export const HELP_SECTIONS = [
  "Account",
  "LLM",
  "Memory",
  "Language",
  "GitHub App",
  "Repository settings",
  "Dashboard",
  "Access tokens",
] as const;

/** A section of the guide. */
export type HelpSection = (typeof HELP_SECTIONS)[number];

/** Help of one option. */
export interface SettingHelp {
  section: HelpSection;
  /** The option's name as the page shows it. */
  title: string;
  what: string;
  /** When the tool reads it. */
  when: string;
  /** What happens when it is empty, or its default. */
  empty: string;
  /** Limits and validation, if any. */
  limits?: string;
  example: string;
  /** Further paragraphs of the guide (not in the tip). */
  details?: string[];
  /** False for a topic the page has no control for (it appears in the guide only). */
  onPage?: false;
}

/** The built-in release branch patterns (server `BUILTIN_BRANCH_PATTERNS`). */
export const BUILT_IN_BRANCH_TEXT = "main, master, release/*, hotfix/*, vX.Y, vX.Y-dev";

/** Name of the dashboard group for owners listed when GitHub search refuses an account. */
export const SEARCH_REFUSED_GROUP = "When GitHub search refuses an account";

const HELP = {
  sign_out: {
    section: "Account",
    title: "Sign out",
    what: "Ends your session in this browser. Other browsers stay signed in.",
    when: "When you press Sign out.",
    empty:
      "Your unsent comment drafts in this browser are removed. Your GitHub pending review stays on GitHub.",
    example: "Sign out on a shared computer before you leave it.",
    details: [
      "With the automatic refresh on and a time zone stored, the server keeps your GitHub sign-in after your last session ends, so the daily refresh can still scan. It keeps it for up to 30 days after your last sign-in; after that the automatic refresh says it needs you to sign in again.",
      "Personal access tokens keep working after Sign out.",
    ],
  },
  sign_out_everywhere: {
    section: "Account",
    title: "Sign out everywhere",
    what: "Ends every session of yours, revokes all your personal access tokens and deletes the GitHub sign-in the server keeps for the automatic refresh.",
    when: "Only through the API (POST /auth/logout with everywhere set); the Settings page has no button for it.",
    empty: "Until you sign in again, the automatic refresh records that it needs you to sign in.",
    example: "Use it when a token or a browser session may have leaked.",
    onPage: false,
  },
  llm_model: {
    section: "LLM",
    title: "Model",
    what: "The LLM model of every job on this installation: analyses, Ask answers and the ranking of My work. One choice for everyone.",
    when: "Each new job reads the current choice; a running job keeps the one it started with.",
    empty: "Sol 6.1 until someone saves another model.",
    limits: "One of the listed models.",
    example: "Pick Astra 6 for a hard pull request, then switch back.",
  },
  llm_effort: {
    section: "LLM",
    title: "Reasoning effort",
    what: "How much the model reasons in analysis jobs: the plan, the PR context and each flow.",
    when: "Each new analysis job reads it.",
    empty: "High until someone saves another effort.",
    limits: "Low, medium or high. Higher effort can take longer.",
    example: "Medium is often enough for small pull requests.",
  },
  ask_effort: {
    section: "LLM",
    title: "Ask effort",
    what: "How much the model reasons when it answers a question you ask about a step.",
    when: "Each new Ask answer reads it. It applies to everyone on this installation.",
    empty: "Medium until someone saves another effort.",
    limits: "Low, medium or high. Lower effort answers sooner.",
    example: "Low for quick questions such as where a value is set.",
  },
  judge_effort: {
    section: "LLM",
    title: "Dashboard ranking effort",
    what: "How much the model reasons when it ranks the items of My work.",
    when: "Each ranking job of a refresh reads it. It applies to everyone on this installation.",
    empty: "Low until someone saves another effort.",
    limits: "Low, medium or high.",
    example: "Medium if the ranking reasons look too thin.",
  },
  llm_status: {
    section: "LLM",
    title: "LLM status",
    what: "The outcome of the most recent LLM call and its time, and the tokens the provider reported for your use.",
    when: "Updated by every LLM call. Read-only.",
    empty: "Not known yet until the first analysis calls the LLM.",
    example: "After a failed call the provider's message stands below the status.",
  },
  memcan_status: {
    section: "Memory",
    title: "Memory",
    what: "Whether MemCan, the memory service, is configured and reachable. Analyses read project knowledge from it and My work reads TODOs from it.",
    when: "Shown as it was last checked. Read-only.",
    empty: "Without MemCan, analyses run without it and My work shows no TODOs.",
    example: "Not configured: the server has no MemCan address.",
  },
  language: {
    section: "Language",
    title: "Language of LLM texts",
    what: "The language of explanations, questions and assessments the LLM writes for you.",
    when: "Read when an analysis starts; an analysis keeps the language it started with, its Ask answers too.",
    empty: "English.",
    limits: "English or Polski.",
    example: "Polski, then analyse the pull request again to get its texts in Polish.",
  },
  app_installations: {
    section: "GitHub App",
    title: "GitHub App",
    what: "The installations of the smart-review GitHub App your GitHub sign-in can see. Comments go into your pending review only where the App is installed.",
    when: "Read when Settings opens. Read-only; install or change the App on GitHub.",
    empty: "No installation: pull requests can be analysed, but comments must be written on GitHub.",
    example: "Install the App on dashpay to comment on dashpay/platform from here.",
  },
  repository_settings: {
    section: "Repository settings",
    title: "Repository settings",
    what: "Read-only facts about one repository: the built-in rules that put tests, docs and generated files under Not analysed, the related repositories and where the project profile comes from.",
    when: "Shown for repositories that have an analysed pull request.",
    empty: "No repository is listed before the first analysis.",
    example: "dashpay/platform: related repositories found in Cargo.toml and Cargo.lock.",
  },
  linked_logins: {
    section: "Dashboard",
    title: "Linked logins",
    what: "Other GitHub accounts of yours. Their pull requests, review requests, assigned and mentioning issues count as yours, and their approvals and ownership count as yours.",
    when: "Every refresh searches them together with your own login.",
    empty: "Only your own login is scanned; it is always scanned.",
    limits: "Up to 5 GitHub logins (letters, digits, - or _, up to 39); not your own login.",
    example:
      "A bot account that opens pull requests you take care of (e.g. Claudius-Maginificent): put it here so its pull requests count as yours, and keep it in Known bots so its reviews count as a bot's. Do not put it into Excluded authors.",
  },
  repo: {
    section: "Dashboard",
    title: "Repository",
    what: "A repository you work in, as owner/name. The row holds that repository's short name, issue watch, ownership policy and release branches; the repository name is also read as a MemCan project for TODOs.",
    when: "Every refresh. Your pull requests and issues are found by search even without a row.",
    empty: "No rows: only search results, without short names, watched issues or policies.",
    limits: "Up to 50 rows, each repository once.",
    example: "dashpay/platform.",
  },
  alias: {
    section: "Dashboard",
    title: "Short name",
    what: "The prefix of the repository's item ids on My work and of the tab title of its reviews.",
    when: "Whenever an item or a review of the repository is shown.",
    empty: "The repository name: platform#1187 instead of PLAT-1187.",
    limits:
      "Capital letters, digits and -, starting with a letter or digit, up to 12; unique among the rows.",
    example: "DET for dashpay/dash-evo-tool: items show as DET-512, the review tab as DET#512.",
  },
  watch_issues: {
    section: "Dashboard",
    title: "Watch issues",
    what: "Also lists every open issue of the repository, not only the issues assigned to you or mentioning you.",
    when: "Every refresh searches the open issues of the watched repositories.",
    empty: "Off: only issues assigned to you or mentioning you.",
    example: "On for a small repository where you triage all issues.",
  },
  ownership_policy: {
    section: "Dashboard",
    title: "Ownership policy",
    what: "Optional. A JSON file listing the paths you own in this repository, in the format of dashpay/stale_prs_are_bad. In a repository with a policy, a pull request stays in Needs you only when it changes a fair amount of code in your paths.",
    when: "Every refresh reads the file for the repository's pull requests.",
    empty: "Leave both fields empty when the repository has no such file: every open pull request counts.",
    limits:
      "Both fields or neither. The repository as owner/name; the path relative, ending with .json, without .., % or \\.",
    example: "Repository dashpay/stale_prs_are_bad, path policies/platform.json.",
    details: [
      "Format: the JSON files of https://github.com/dashpay/stale_prs_are_bad (folder policies/). The tool reads only areas: a list of objects with paths, owners and reviewers. The paths of every area that names you or a linked login as owner or reviewer are yours; a path matches every file that starts with it.",
      "What counts: a pull request whose changes in your paths are only comments or docs, or only a few code lines (fewer than 12 distinct code lines, or fewer than 8 in any one file), is left out of Needs you. This applies to your own pull requests as well as to those of others.",
      "When the file cannot be read: its pull requests stay in the list with ownership unverified, the LLM is asked to confirm, and What the scan missed names the file.",
      "Worked examples: dashpay/platform uses dashpay/stale_prs_are_bad with policies/platform.json; dashpay/tenderdash uses policies/tenderdash.json; dashpay/dash-evo-tool uses policies/dash-evo-tool.json. A repository without a policy file: leave both fields empty.",
    ],
  },
  branch_patterns: {
    section: "Dashboard",
    title: "Release branches",
    what: "Names of the repository's release and long-lived branches. A pull request based on such a branch is not treated as stacked on another pull request, and one that targets such a branch (other than the default branch) counts as targeting a release line.",
    when: "Every ranking of the repository's pull requests.",
    empty: `Leave empty for the built-ins: ${BUILT_IN_BRANCH_TEXT}. Your list replaces the built-ins; it does not add to them.`,
    limits:
      "Up to 10 patterns, separated by spaces or commas; letters, digits, . _ / - and *. X.Y stands for a version of two or more numbers (1.8, 5.1, 2.0.1); * matches any text.",
    example: "vX.Y-dev matches v1.8-dev and v5.1-dev; release/* matches release/2.1; v*-dev works too.",
  },
  enumerate_owners: {
    section: "Dashboard",
    title: SEARCH_REFUSED_GROUP,
    what: "Optional. Organisations or users whose open pull requests and issues are listed directly when GitHub search will not answer for one of your accounts. GitHub search refuses some logins, typically bot or restricted accounts.",
    when: "Only in a refresh where search refuses one of your logins. The scan then lists the open pull requests and issues of every repository of these owners and keeps those involving the refused login.",
    empty: "Nothing is listed; the refused account's work is missing from My work.",
    limits: "Up to 5 owners. Each costs extra GitHub requests per refresh, so keep the list short.",
    example: "dashpay.",
    details: ["How you find out: after a refresh, What the scan missed names the accounts search refused."],
  },
  lookback_days: {
    section: "Dashboard",
    title: "Look-back days",
    what: "How many days an issue counts as new.",
    when: "Stored with your settings. The current server does not use it in the scan or the ranking yet.",
    empty: "7 days.",
    limits: "A whole number from 1 to 30.",
    example: "14 if you look at My work once a week.",
  },
  ready_label: {
    section: "Dashboard",
    title: "Ready for a human label",
    what: "The label that marks a pull request ready for a human review.",
    when: "Every ranking: a pull request with this label ranks higher, and one of yours with it, green checks and no open threads counts as ready for others.",
    empty: "ready-for-human.",
    limits: "1 to 50 characters; compared without regard to case.",
    example: "needs-review.",
  },
  skip_authors: {
    section: "Dashboard",
    title: "Excluded authors",
    what: "Banned authors. Pull requests and issues written by these accounts are never shown in Needs you — also when a review is requested from you or they are assigned to you. Being listed does not make an account a bot.",
    when: "Only on My work (the scan and the ranking); never in pull request analyses, flows or Ask.",
    empty: "dependabot, dependabot[bot].",
    limits: "Up to 20 GitHub logins, optionally ending in [bot].",
    example: "Add renovate[bot] to hide Renovate's pull requests.",
  },
  known_bots: {
    section: "Dashboard",
    title: "Known bots",
    what: "Accounts treated as bots: their review requests and comments do not count as a person waiting for you, they are not listed among reviewers you wait on, and their reviews count as bot reviews. GitHub Apps (…[bot]) are always bots and need not be listed. Nothing else is built in.",
    when: "Only on My work (the scan and the ranking); never in pull request analyses, flows or Ask.",
    empty:
      "thepastaclaw, Claudius-Maginificent and DCG-Claude. An empty list leaves only GitHub Apps as bots.",
    limits: "Up to 20 GitHub logins, optionally ending in [bot]. Your own and linked logins may be listed.",
    example:
      "Add a review bot that runs under a normal user account, so its review requests do not wait on you.",
  },
  include_todos: {
    section: "Dashboard",
    title: "Show my MemCan TODOs",
    what: "Lists your open MemCan TODOs on My work next to pull requests and issues.",
    when: "Every refresh reads them from the MemCan projects below. Switching it off hides them at once.",
    empty: "On. Without MemCan on the server there are no TODOs and the switch is off.",
    example: "Off if you keep your TODOs elsewhere.",
    details: [
      "When MemCan cannot be reached, the TODO rows of the earlier scan stay and each says not refreshed.",
    ],
  },
  memcan_extra_projects: {
    section: "Dashboard",
    title: "Extra MemCan projects",
    what: "MemCan projects whose TODOs are read besides the projects named after your repository rows.",
    when: "Every refresh with TODOs on.",
    empty: "Only the repository rows' names are read (dashpay/platform gives platform).",
    limits: "Up to 10 names (letters, digits, . _ or -, up to 100).",
    example: "home-infra.",
  },
  schedule_enabled: {
    section: "Dashboard",
    title: "Refresh my dashboard automatically every day",
    what: "Runs one refresh a day at the time below, with your GitHub sign-in.",
    when: "The server checks every minute; it starts your run up to 9 minutes after the time. A run missed (server stopped) is caught up once on the same day; a manual refresh after the time counts for the day.",
    empty: "On. It needs a time zone; without one it does not run.",
    example: "On, 06:00: My work is ready when you start.",
    details: [
      "While it is on, the server keeps your GitHub sign-in for up to 30 days after your last sign-in. When the sign-in has ended, My work says the automatic refresh needs you to sign in again.",
    ],
  },
  schedule_time: {
    section: "Dashboard",
    title: "Time of day",
    what: "The local time of the daily refresh.",
    when: "Every day in your time zone.",
    empty: "06:00.",
    limits: "HH:MM in steps of 15 minutes.",
    example: "07:30.",
  },
  time_zone: {
    section: "Dashboard",
    title: "Time zone",
    what: "Your time zone for the daily refresh, the day of the daily ranking limit and the history charts.",
    when: "Your first visit to My work stores your browser's zone when none is stored.",
    empty: "Not set yet: the automatic refresh does not run.",
    limits: "A time zone name such as Europe/Warsaw.",
    example: "Europe/Warsaw.",
  },
  ranking_limits: {
    section: "Dashboard",
    title: "Ranking limits",
    what: "The LLM judges at most 48 items per refresh and 120 per day for each user.",
    when: "Every ranking. Items not judged again because they did not change cost nothing.",
    empty: "Fixed limits; there is no setting.",
    example: "A first refresh with 150 items judges 48; the rest stay provisional until later refreshes.",
    onPage: false,
  },
  access_tokens: {
    section: "Access tokens",
    title: "Access tokens",
    what: "Personal access tokens let your agents read over HTTP what you can read here: My work, reviews and their questions. They are read-only.",
    when: "When an agent sends Authorization: Bearer srp_… with a request.",
    empty: "No tokens: agents cannot read your data.",
    limits:
      "Name of 1 to 60 characters; lifetime of 30, 90 (default) or 365 days; at most 10 live tokens. The secret is shown once.",
    example: "A token named laptop-agent for 90 days. The agent API: docs/contracts/agent-api.md.",
    details: ["Revoking a token stops it at once. Sign out everywhere revokes every token."],
  },
} satisfies Record<string, SettingHelp>;

/** An option with help. */
export type HelpTopic = keyof typeof HELP;

/** Help of every option. */
export const SETTINGS_HELP: Record<HelpTopic, SettingHelp> = HELP;

/** Topics in guide order (section, then map order). */
export const HELP_TOPICS: HelpTopic[] = HELP_SECTIONS.flatMap((section) =>
  (Object.keys(HELP) as HelpTopic[]).filter((topic) => HELP[topic].section === section),
);

/** Where each field of the settings contracts is explained; a new field without help breaks the type check. */
export const CONTRACT_HELP: {
  settings: Record<keyof Settings, HelpTopic>;
  llmSelection: Record<keyof LlmSelection, HelpTopic>;
  dashboard: Record<keyof DashboardSettings, HelpTopic>;
  repository: Record<keyof RepositoryRule, HelpTopic>;
  accessToken: Record<keyof CreateAccessToken, HelpTopic>;
} = {
  settings: {
    llm_selection: "llm_model",
    llm_usage: "llm_status",
    llm_status: "llm_status",
    memcan_status: "memcan_status",
    language: "language",
    app_installations: "app_installations",
    repos: "repository_settings",
  },
  llmSelection: {
    models: "llm_model",
    efforts: "llm_effort",
    current: "llm_model",
    judge_effort: "judge_effort",
    ask_effort: "ask_effort",
    changed_by: "llm_model",
    changed_at: "llm_model",
  },
  dashboard: {
    linked_logins: "linked_logins",
    repositories: "repo",
    enumerate_owners: "enumerate_owners",
    lookback_days: "lookback_days",
    ready_label: "ready_label",
    skip_authors: "skip_authors",
    known_bots: "known_bots",
    include_todos: "include_todos",
    memcan_extra_projects: "memcan_extra_projects",
    schedule_enabled: "schedule_enabled",
    schedule_time: "schedule_time",
    time_zone: "time_zone",
  },
  repository: {
    repo: "repo",
    alias: "alias",
    watch_issues: "watch_issues",
    ownership_policy: "ownership_policy",
    branch_patterns: "branch_patterns",
  },
  accessToken: { name: "access_tokens", scopes: "access_tokens", expires_in_days: "access_tokens" },
};

/** The in-app guide's address of `topic`. */
export const helpHref = (topic: HelpTopic) => `/settings/help#${topic}`;

/** One line of the tip and the guide: label and text. */
export function helpLines(help: SettingHelp): [string, string][] {
  return [
    ["What", help.what],
    ["When used", help.when],
    ["Empty or default", help.empty],
    ...(help.limits ? [["Limits", help.limits] as [string, string]] : []),
    ["Example", help.example],
  ];
}

/** `text` as Markdown text: `*` and `\` stay literal. */
const markdownText = (text: string) => text.replace(/[\\*]/g, "\\$&");

/** docs/settings.md: the guide as Markdown. */
export function settingsHelpMarkdown(): string {
  const out = [
    "# Settings guide",
    "",
    "<!-- Generated from web/src/components/settings/settingsHelp.ts; the in-app guide at /settings/help shows",
    "the same text. Update: UPDATE_GENERATED=1 npx vitest run src/components/settings/settingsHelp.test.ts -->",
    "",
    "Every option on the Settings page: what it is, when smart-review uses it, what happens when it is empty, its limits and an example. The `?` next to an option shows the same text.",
  ];
  for (const section of HELP_SECTIONS) {
    out.push("", `## ${section}`);
    for (const topic of HELP_TOPICS.filter((t) => SETTINGS_HELP[t].section === section)) {
      const help = SETTINGS_HELP[topic];
      out.push("", `### ${help.title}`, "");
      for (const [label, text] of helpLines(help)) out.push(`- **${label}:** ${markdownText(text)}`);
      for (const paragraph of help.details ?? []) out.push("", markdownText(paragraph));
    }
  }
  return `${out.join("\n")}\n`;
}
