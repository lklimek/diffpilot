import { HttpResponse } from "msw";
import type { LlmLanguage } from "../../api/generated/types";
import { scenario, SETTINGS } from "../scenarios";
import { route } from "./common";

let language: LlmLanguage | null = null;

/** Stores the language set through `PUT /api/v1/settings/language` (null = the scenario's). */
export function setMockLanguage(value: LlmLanguage | null): void {
  language = value;
}

/** `GET /api/v1/settings`. */
export const getSettings = route("getSettings", () => {
  const settings = scenario().settings ?? SETTINGS;
  return HttpResponse.json({ ...settings, language: language ?? settings.language });
});
