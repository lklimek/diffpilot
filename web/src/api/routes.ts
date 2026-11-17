import { routes as generated } from "./generated/routes";

// TODO(WP3-MEMCAN merge): drop once the generated routes contain `deleteFeedback` (docs/contracts/http-api.md).
const documented = {
  deleteFeedback: {
    method: "DELETE",
    path: "/api/v1/analyses/{id}/feedback/{feedback_id}",
    auth: "session_csrf",
  },
} as const;

/** Browser-facing routes: the generated ones plus documented routes whose generated entry is not merged yet. */
export const routes = { ...generated, ...documented } as const;

/** Name of a browser-facing route. */
export type RouteName = keyof typeof routes;
