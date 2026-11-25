import { savedSelection, resetSelection } from "./set-llm";
import { resetControlledOwners, savedControlledOwners } from "./set-controlled-owners";
import { HttpResponse } from "msw";
import type { LlmLanguage, Settings } from "../../api/generated/types";
import { scenario, SETTINGS } from "../scenarios";
import { route } from "./common";

let language: LlmLanguage | null = null;

/** Stores the language set through `PUT /api/v1/settings/language` (null = the scenario's). */
export function setMockLanguage(value: LlmLanguage | null): void {
  language = value;
  if (value === null) {
    resetSelection();
    resetControlledOwners();
  }
}

/** `GET /api/v1/settings`. */
export const getSettings = route("getSettings", () => {
  const settings = scenario().settings ?? SETTINGS;
  return HttpResponse.json<Settings>({
    ...settings,
    ...(scenario().settingsV14 !== "unconfigured" && {
      llm_usage: {
        today: null,
        last_7_days: null,
        ...settings.llm_usage,
        this_month: {
          input_tokens: 7_100_000,
          cached_input_tokens: 5_600_000,
          output_tokens: 310_000,
          reasoning_output_tokens: null,
          jobs: 67,
        },
      },
    }),
    llm_selection: savedSelection ?? settings.llm_selection,
    language: language ?? settings.language,
    controlled_owners: savedControlledOwners() ??
      settings.controlled_owners ?? { owners: [], changed_by: null, changed_at: null },
  });
});
