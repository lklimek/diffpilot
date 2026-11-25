// TODO(v1.4 phase B): replace with generated contract type (plan C3)
import type { RepoRef } from "../generated/types";
import { settingsResponse } from "./settingsResponse";

export const REPOSITORY_INSTALLATIONS_PATH = "/api/v1/settings/repository-installations";

export interface RepositoryInstallations {
  repositories: { repo: RepoRef; live: boolean }[];
}

export async function fetchRepositoryInstallations(
  signal?: AbortSignal,
): Promise<RepositoryInstallations | null> {
  return settingsResponse(
    await fetch(REPOSITORY_INSTALLATIONS_PATH, { headers: { Accept: "application/json" }, signal }),
  );
}
