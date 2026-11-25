// TODO(v1.4 phase B): replace with generated contract type (plan C13)
import { settingsResponse } from "./settingsResponse";

export const GITHUB_WEBHOOK_PATH = "/api/v1/settings/github-webhook";

export interface GitHubWebhook {
  webhook_url: string | null;
}

export async function fetchGitHubWebhook(signal?: AbortSignal): Promise<GitHubWebhook | null> {
  return settingsResponse(
    await fetch(GITHUB_WEBHOOK_PATH, { headers: { Accept: "application/json" }, signal }),
  );
}
