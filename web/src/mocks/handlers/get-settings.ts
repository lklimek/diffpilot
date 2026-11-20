import { savedSelection, resetSelection } from "./set-llm";
import { resetControlledOwners, savedControlledOwners } from "./set-controlled-owners";
import { HttpResponse } from "msw";
import type { LlmLanguage } from "../../api/generated/types";
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
  return HttpResponse.json({
    ...settings,
    llm_selection: savedSelection ?? settings.llm_selection,
    language: language ?? settings.language,
    controlled_owners: savedControlledOwners() ?? settings.controlled_owners,
  });
});
