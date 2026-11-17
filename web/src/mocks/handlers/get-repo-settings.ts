import { HttpResponse } from "msw";
import { REPO_SETTINGS, scenario } from "../scenarios";
import { apiError, route } from "./common";

/** `GET /api/v1/repos/{owner}/{name}/settings`: acme/node-client only. */
export const getRepoSettings = route("getRepoSettings", ({ params }) => {
  const chosen = scenario().repoSettings;
  if (chosen === null) {
    return apiError(
      409,
      "analysis_not_finished",
      "Not known yet: the repository settings are read during an analysis and appear once one has run.",
    );
  }
  const settings = chosen ?? REPO_SETTINGS;
  if (params.owner !== settings.repo.owner || params.name !== settings.repo.name) {
    return apiError(404, "not_found", "This repository is not known.");
  }
  return HttpResponse.json(settings);
});
