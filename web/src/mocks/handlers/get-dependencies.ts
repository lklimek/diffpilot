import { HttpResponse } from "msw";
import { mockDependencies } from "../dependencies";
import { scenario } from "../scenarios";
import { apiError, route, forAnalysis } from "./common";
import { savedControlledOwners } from "./set-controlled-owners";

/** `GET /api/v1/analyses/{id}/dependencies` (R15): the Cargo map, facts and crate notes. */
export const getDependencies = route(
  "getDependencies",
  forAnalysis(() => {
    const view = (scenario().dependencies ?? mockDependencies)();
    if (view === "error") {
      return apiError(503, "github_unavailable", "The repository could not be read. Try again in a minute.");
    }
    // As the server: "yours" is applied at read time from the PR's owner and the saved Settings list.
    const saved = savedControlledOwners();
    if (saved) {
      const prOwner = view.repositories.find((r) => r.kind === "this")?.repo?.owner;
      const all = [...(prOwner ? [prOwner] : []), ...saved.owners];
      const owners = [...new Map(all.map((o) => [o.toLowerCase(), o])).values()];
      const lower = owners.map((o) => o.toLowerCase());
      view.controlled_owners = owners.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
      for (const repo of view.repositories)
        if (repo.repo) repo.controlled = lower.includes(repo.repo.owner.toLowerCase());
    }
    return HttpResponse.json(view);
  }),
);
