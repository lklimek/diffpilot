import { route, seedJson } from "./common";

/** `POST /api/v1/pr/resolve`: always the fixture PR (no analysis, or the finished one in `resume`). */
export const resolvePr = route("resolvePr", seedJson("pr-resolve"));
