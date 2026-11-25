// TODO(v1.4 phase B): replace with generated contract type (plan C6)
import type { UsageTotals } from "../generated/types";
import { settingsResponse } from "./settingsResponse";

export const MONTHLY_TOKEN_USAGE_PATH = "/api/v1/settings/monthly-token-usage";

export interface MonthlyTokenUsage {
  this_month: UsageTotals | null;
}

export async function fetchMonthlyTokenUsage(signal?: AbortSignal): Promise<MonthlyTokenUsage | null> {
  return settingsResponse(
    await fetch(MONTHLY_TOKEN_USAGE_PATH, { headers: { Accept: "application/json" }, signal }),
  );
}
