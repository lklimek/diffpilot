// In-app paths of the navigation shell (plan §F.1, §F.2).

/** My work, the start page. */
export const MY_WORK_PATH = "/";

/** The review start page ("Review a PR"); `/?pr=` and `/?login_error=` redirect here with their query. */
export const REVIEW_START_PATH = "/review";

/** Settings. */
export const SETTINGS_PATH = "/settings";
/** The settings guide. */
export const SETTINGS_HELP_PATH = "/settings/help";

/** The usage and limits card in Settings. */
export const LLM_USAGE_SECTION = "llm-usage";
export const SETTINGS_PAGES = [
  { id: "account", label: "Account", href: "/settings/account" },
  { id: "llm", label: "LLM & usage", href: "/settings/llm" },
  { id: "github-sync", label: "GitHub sync", href: "/settings/github-sync" },
  { id: "repositories", label: "Repositories", href: "/settings/repositories" },
  { id: "review-loop", label: "Review loop", href: "/settings/review-loop" },
  { id: "memcan", label: "MemCan", href: "/settings/memcan" },
] as const;
export type SettingsPage = (typeof SETTINGS_PAGES)[number]["id"];

export const LLM_USAGE_PATH = "/settings/llm";
export const SETTINGS_NOTIFICATIONS_PATH = "/settings/review-loop#notifications";
