// TODO(v1.4 phase B): replace with generated contract type (plan C8)
import { api } from "../client";
import { settingsResponse } from "./settingsResponse";

export const RANKING_USAGE_PATH = "/api/v1/settings/ranking-usage";
export const RESET_RANKING_USAGE_PATH = `${RANKING_USAGE_PATH}/reset`;

export interface RankingUsage {
  sent_today: number;
  daily_cap: number;
  per_refresh_cap: number;
  resets_at: string;
  left_provisional: number;
}

export async function fetchRankingUsage(signal?: AbortSignal): Promise<RankingUsage | null> {
  return settingsResponse(
    await fetch(RANKING_USAGE_PATH, { headers: { Accept: "application/json" }, signal }),
  );
}

export async function resetRankingUsage(): Promise<RankingUsage> {
  const { csrf_token } = await api.getMe();
  if (!csrf_token) throw new Error("Sign in again to reset your limits.");
  const usage = await settingsResponse<RankingUsage>(
    await fetch(RESET_RANKING_USAGE_PATH, {
      method: "POST",
      headers: { Accept: "application/json", "X-CSRF-Token": csrf_token },
    }),
  );
  if (!usage) throw new Error("Reset my limits is unavailable.");
  return usage;
}
