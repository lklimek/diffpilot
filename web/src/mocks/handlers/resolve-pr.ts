import { HttpResponse } from "msw";
import type { ResolvePrRequest, ResolvePrResponse } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { scenario } from "../scenarios";
import { apiError, route } from "./common";

const PR_URL = /^https:\/\/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)\/?$/;

/**
 * `POST /api/v1/pr/resolve`: the fixture PR (no analysis, or the finished one in `resume`, changed by the
 * scenario); `acme/private-repo` is private; any other PR does not exist; a non-PR URL is `not_a_pr_url`.
 */
export const resolvePr = route("resolvePr", async ({ request }) => {
  const { url } = (await request.json()) as ResolvePrRequest;
  const match = PR_URL.exec(url.trim());
  if (!match) {
    return apiError(
      400,
      "not_a_pr_url",
      "This is not a GitHub pull request URL. Use a link like https://github.com/owner/repo/pull/123.",
    );
  }
  const [, owner, name, number] = match;
  if (owner === "acme" && name === "private-repo") {
    return apiError(
      422,
      "private_repository",
      "This repository is private. Only public repositories can be analysed.",
    );
  }
  const seed = seedNow("pr-resolve") as ResolvePrResponse;
  const { repo } = seed.pr;
  if (owner !== repo.owner || name !== repo.name || Number(number) !== seed.pr.number) {
    return apiError(404, "not_found", "This pull request does not exist or is not visible.");
  }
  return HttpResponse.json(scenario().resolve?.(seed) ?? seed);
});
