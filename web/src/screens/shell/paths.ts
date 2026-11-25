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
export const LLM_USAGE_PATH = `${SETTINGS_PATH}#${LLM_USAGE_SECTION}`;
