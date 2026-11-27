import { HttpResponse } from "msw";
import type { GithubWebhook, RankingUsage, RepositoryInstallations } from "../../api/generated/types";
import { route } from "./common";
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
  route("getRankingUsage", () =>
    scenario().settingsV14 === "unavailable"
      ? unavailable()
      : HttpResponse.json(
          afterReset ??
            (scenario().settingsV14 === "at-limit" ? { ...ranking, sent_today: ranking.daily_cap } : ranking),
        ),
  ),
  route("resetRankingUsage", ({ request }) => {
    if (!request.headers.get("X-CSRF-Token")) return new HttpResponse(null, { status: 403 });
    if (scenario().settingsV14 === "reset-fails") return unavailable();
    afterReset = { ...ranking, sent_today: 0 };
    return HttpResponse.json(afterReset);
  }),
  route("getGithubWebhook", () =>
    scenario().settingsV14 === "unavailable"
      ? unavailable()
      : HttpResponse.json<GithubWebhook>({
          configured: scenario().settingsV14 !== "unconfigured",
          last_delivery_at: scenario().settingsV14 === "unconfigured" ? null : "2026-11-25T09:00:00Z",
          dropped_deliveries: 0,
          refused_connections: 0,
          webhook_url:
            scenario().settingsV14 === "unconfigured"
              ? null
              : "https://webhooks.manago.cloud/diffpilot/github",
        }),
  ),
  route("getRepositoryInstallations", () =>
    scenario().settingsV14 === "unavailable"
      ? unavailable()
      : HttpResponse.json<RepositoryInstallations>({
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
];
