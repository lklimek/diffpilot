import { HttpResponse } from "msw";
import { fixtureState, linesFor } from "../fixtures";
import { apiError, forAnalysis, route } from "./common";

const MAX_LINES = 2000;

/** `GET /api/v1/analyses/{id}/lines`: slices the whole-file seed per http-api.md "Ranges". */
export const getLines = route(
  "getLines",
  forAnalysis(({ request }) => {
    const query = new URL(request.url).searchParams;
    const path = query.get("path");
    const side = query.get("side");
    const from = Number(query.get("from"));
    const to = Number(query.get("to"));
    if (!path || (side !== "head" && side !== "base") || !Number.isInteger(from) || !Number.isInteger(to)) {
      return apiError(400, "invalid_request", "The line range is malformed.");
    }
    if (from < 1 || to < from) return apiError(400, "invalid_request", "The line range is malformed.");
    if (to - from + 1 > MAX_LINES) {
      return apiError(400, "range_too_large", `At most ${MAX_LINES} lines can be shown at once.`);
    }
    const answer = linesFor(fixtureState(), path, side, from, to);
    if (!answer) return apiError(404, "not_found", "This file is not part of the analysis.");
    return HttpResponse.json(answer);
  }),
);
