import { HttpResponse, delay } from "msw";
import type { SetViewedRequest, Tour } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { mockFiles } from "../files";
import { updateStore } from "../review-store";
import { scenario } from "../scenarios";
import { apiError, forAnalysis, route } from "./common";

/** How long GitHub takes to confirm a Viewed mark in the mocks. */
export const VIEWED_MS = 300;

// Status and message as the server sends them (crates/server/src/api/files.rs).
const ERRORS = {
  github_refused: [
    502,
    "Not saved on GitHub: GitHub refused the change. Your mark is unchanged; mark it on GitHub.",
  ],
  pr_head_changed: [
    409,
    "The pull request changed since this analysis; marking Viewed would apply to newer code. Analyse it again, or mark it on GitHub.",
  ],
  github_unavailable: [502, "GitHub did not confirm the change. Reload to see your mark, then try again."],
} as const;

/** `PUT /api/v1/analyses/{id}/files/viewed` (R13): 204 once "GitHub" confirmed; nothing is stored on refusal. */
export const setFileViewed = route(
  "setFileViewed",
  forAnalysis(async ({ request }) => {
    const { path, viewed } = (await request.json()) as SetViewedRequest;
    if (!mockFiles(seedNow("tour") as Tour).files.some((f) => f.path === path)) {
      return apiError(400, "invalid_request", "This file is not part of the pull request.");
    }
    await delay(VIEWED_MS);
    const failure = scenario().viewedError;
    if (failure) return apiError(ERRORS[failure][0], failure, ERRORS[failure][1]);
    updateStore((store) => {
      store.viewed = { ...store.viewed, [path]: viewed ? "viewed" : "unviewed" };
    });
    return new HttpResponse(null, { status: 204 });
  }),
);
