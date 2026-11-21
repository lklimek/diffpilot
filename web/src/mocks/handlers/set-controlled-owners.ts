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

const OWNER = /^[A-Za-z0-9][A-Za-z0-9-]{0,38}$/;

/** `PUT /api/v1/settings/controlled-owners` (R16), like the server: trims, answers 400 `invalid_request`, keeps the
 * first spelling of owners that differ only in case. */
export const setControlledOwners = route("setControlledOwners", async ({ request }) => {
  const body = (await request.json()) as Partial<SetControlledOwnersRequest>;
  const owners = Array.isArray(body.owners) ? body.owners.map((owner) => String(owner).trim()) : null;
  if (owners === null || owners.length > 50 || owners.some((owner) => !OWNER.test(owner))) {
    return apiError(
      400,
      "invalid_request",
      "Each organisation must be a GitHub login (letters, digits and -, up to 39 characters); at most 50.",
    );
  }
  const first = new Map<string, string>();
  for (const owner of owners) {
    if (!first.has(owner.toLowerCase())) first.set(owner.toLowerCase(), owner);
  }
  const unique = [...first.values()].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
  saved = { owners: unique, changed_by: "lklimek", changed_at: new Date().toISOString() };
  return HttpResponse.json(saved);
});
