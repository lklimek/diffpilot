import { HttpResponse } from "msw";
import type { LlmSelection, ModelChoice } from "../../api/generated/types";
import { SETTINGS, scenario } from "../scenarios";
import { apiError, route } from "./common";
export let savedSelection: LlmSelection | undefined;
/** Clears the installation choice between mock tests. */
export function resetSelection() {
  savedSelection = undefined;
}
/** Shared mock installation selection. */
export const setLlm = route("setLlm", async ({ request }) => {
  if (scenario().llmSaveFails) return apiError(503, "unavailable", "Try again later.");
  const choice = (await request.json()) as ModelChoice;
  const selection = savedSelection ?? SETTINGS.llm_selection!;
  if (
    !selection.models.some((m) => m.id === choice.model) ||
    !selection.efforts.includes(choice.reasoning_effort)
  )
    return apiError(400, "invalid_request", "Choose an available model and effort.");
  savedSelection = {
    ...selection,
    current: choice,
    changed_by: "alice",
    changed_at: new Date().toISOString(),
  };
  return HttpResponse.json(savedSelection);
});
