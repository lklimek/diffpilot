import { HttpResponse } from "msw";
import { REPO_SETTINGS, scenario } from "../scenarios";
import { apiError, route } from "./common";

/** `GET /api/v1/repos/{owner}/{name}/settings`: acme/node-client only. */
export const getRepoSettings = route("getRepoSettings", ({ params }) => {
  const settings = scenario().repoSettings ?? REPO_SETTINGS;
  if (params.owner !== settings.repo.owner || params.name !== settings.repo.name) {
    return apiError(404, "not_found", "This repository is not known.");
  }
  return HttpResponse.json(settings);
});
