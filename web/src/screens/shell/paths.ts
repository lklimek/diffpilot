// In-app paths of the navigation shell (plan §F.1, §F.2).

/** My work, the start page. */
export const MY_WORK_PATH = "/";

/** The review start page ("Review a PR"); `/?pr=` and `/?login_error=` redirect here with their query. */
export const REVIEW_START_PATH = "/review";

/** Settings. */
export const SETTINGS_PATH = "/settings";
/** The settings guide. */
export const SETTINGS_HELP_PATH = "/settings/help";
export const ACCOUNT_SETTINGS_PATH = "/settings/account";
export const GITHUB_SYNC_SETTINGS_PATH = "/settings/github-sync";
export const REPOSITORY_SETTINGS_PATH = "/settings/repositories";
export const REVIEW_LOOP_SETTINGS_PATH = "/settings/review-loop";
export const LLM_USAGE_PATH = "/settings/llm";

/** The usage and limits card in Settings. */
export const LLM_USAGE_SECTION = "llm-usage";
export const SETTINGS_PAGES = [
  { id: "account", label: "Account", href: ACCOUNT_SETTINGS_PATH },
  { id: "llm", label: "LLM & usage", href: LLM_USAGE_PATH },
  { id: "github-sync", label: "GitHub sync", href: GITHUB_SYNC_SETTINGS_PATH },
  { id: "repositories", label: "Repositories", href: REPOSITORY_SETTINGS_PATH },
  { id: "review-loop", label: "Review loop", href: REVIEW_LOOP_SETTINGS_PATH },
  { id: "memcan", label: "MemCan", href: "/settings/memcan" },
] as const;
export type SettingsPage = (typeof SETTINGS_PAGES)[number]["id"];

export const SETTINGS_NOTIFICATIONS_PATH = `${REVIEW_LOOP_SETTINGS_PATH}#notifications`;
