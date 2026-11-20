import { HttpResponse } from "msw";
import type { ControlledOwners, SetControlledOwnersRequest } from "../../api/generated/types";
import { apiError, route } from "./common";

let saved: ControlledOwners | null = null;

/** The list saved through `PUT /api/v1/settings/controlled-owners` (null = never saved). */
export function savedControlledOwners(): ControlledOwners | null {
  return saved;
}

/** Forgets the saved list (between tests). */
export function resetControlledOwners(): void {
  saved = null;
}

const OWNER = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;

/** `PUT /api/v1/settings/controlled-owners` (R16). */
export const setControlledOwners = route("setControlledOwners", async ({ request }) => {
  const { owners } = (await request.json()) as SetControlledOwnersRequest;
  if (owners.length > 50 || owners.some((owner) => !OWNER.test(owner))) {
    return apiError(422, "invalid_request", "Each owner must be a GitHub login; at most 50.");
  }
  const unique = [...new Map(owners.map((o) => [o.toLowerCase(), o])).values()].sort((a, b) =>
    a.toLowerCase().localeCompare(b.toLowerCase()),
  );
  saved = { owners: unique, changed_by: "lklimek", changed_at: new Date().toISOString() };
  return HttpResponse.json(saved);
});
