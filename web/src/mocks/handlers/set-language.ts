import { HttpResponse } from "msw";
import type { SetLanguageRequest } from "../../api/generated/types";
import { apiError, route } from "./common";
import { setMockLanguage } from "./get-settings";

/** `PUT /api/v1/settings/language`. */
export const setLanguage = route("setLanguage", async ({ request }) => {
  const { language } = (await request.json()) as SetLanguageRequest;
  if (language !== "en" && language !== "pl") {
    return apiError(400, "invalid_request", "This language is not supported.");
  }
  setMockLanguage(language);
  return new HttpResponse(null, { status: 204 });
});
