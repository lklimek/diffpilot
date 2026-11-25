import { http, HttpResponse } from "msw";
import { GITHUB_WEBHOOK_PATH } from "../../api/provisional/githubWebhook";
import { MONTHLY_TOKEN_USAGE_PATH } from "../../api/provisional/monthlyTokenUsage";
import {
  RANKING_USAGE_PATH,
  RESET_RANKING_USAGE_PATH,
  type RankingUsage,
} from "../../api/provisional/rankingUsage";
import { REPOSITORY_INSTALLATIONS_PATH } from "../../api/provisional/repositoryInstallations";
import { scenario } from "../scenarios";

const ranking: RankingUsage = {
  sent_today: 37,
  daily_cap: 120,
  per_refresh_cap: 48,
  resets_at: "2026-11-25T23:00:00Z",
  left_provisional: 3,
};
let afterReset: RankingUsage | undefined;
export function resetSettingsV14() {
  afterReset = undefined;
}
const unavailable = () =>
  HttpResponse.json({ message: "Settings are temporarily unavailable." }, { status: 503 });

export const settingsV14Handlers = [
  http.get(RANKING_USAGE_PATH, () =>
    scenario().settingsV14 === "unavailable"
      ? unavailable()
      : HttpResponse.json(
          afterReset ??
            (scenario().settingsV14 === "at-limit" ? { ...ranking, sent_today: ranking.daily_cap } : ranking),
        ),
  ),
  http.post(RESET_RANKING_USAGE_PATH, ({ request }) => {
    if (!request.headers.get("X-CSRF-Token")) return new HttpResponse(null, { status: 403 });
    if (scenario().settingsV14 === "reset-fails") return unavailable();
    afterReset = { ...ranking, sent_today: 0 };
    return HttpResponse.json(afterReset);
  }),
  http.get(GITHUB_WEBHOOK_PATH, () =>
    scenario().settingsV14 === "unavailable"
      ? unavailable()
      : HttpResponse.json({
          webhook_url:
            scenario().settingsV14 === "unconfigured"
              ? null
              : "https://webhooks.manago.cloud/diffpilot/github",
        }),
  ),
  http.get(REPOSITORY_INSTALLATIONS_PATH, () =>
    scenario().settingsV14 === "unavailable"
      ? unavailable()
      : HttpResponse.json({
          repositories:
            scenario().settingsV14 === "unconfigured" || scenario().settings?.app_installations?.length === 0
              ? []
              : [
                  { repo: { owner: "acme", name: "node-client" }, live: true },
                  { repo: { owner: "acme", name: "live-only" }, live: true },
                  { repo: { owner: "acme", name: "platform" }, live: false },
                ],
        }),
  ),
  http.get(MONTHLY_TOKEN_USAGE_PATH, () =>
    HttpResponse.json({
      this_month:
        scenario().settingsV14 === "unconfigured"
          ? null
          : {
              input_tokens: 7_100_000,
              cached_input_tokens: 5_600_000,
              output_tokens: 310_000,
              reasoning_output_tokens: null,
              jobs: 67,
            },
    }),
  ),
];
