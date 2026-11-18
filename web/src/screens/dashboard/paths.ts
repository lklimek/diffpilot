// In-app paths of the navigation shell (plan §F.1, §F.2) during the dark launch.

/** My work. WP11-SWITCH moves it to `/`. */
export const MY_WORK_PATH = "/my-work";

/**
 * The review start page. Until WP11-SWITCH it is Home at `/`, which fills its field from `?pr=`; the switch moves
 * it to `/review` and keeps `/?pr=` working as a redirect.
 */
export const REVIEW_START_PATH = "/";

/** Settings. */
export const SETTINGS_PATH = "/settings";
