import { HttpResponse } from "msw";
import type { MeResponse } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { scenario } from "../scenarios";
import { route } from "./common";

// Set by `POST /auth/logout` until the mocks are reset (tests) or the page reloads.
let signedOut = false;

/** Signed in again (tests). */
export function resetSignOut(): void {
  signedOut = false;
}

/** `GET /api/v1/me`: the signed-in fixture user with its CSRF token (scenario `signed-out`, or after logout: nobody). */
export const getMe = route("getMe", () =>
  HttpResponse.json(
    signedOut ? { user: null, csrf_token: null } : (scenario().me?.() ?? (seedNow("me") as MeResponse)),
  ),
);

/** `POST /auth/logout`: ends the mock session. */
export const authLogout = route("authLogout", () => {
  signedOut = true;
  return new HttpResponse(null, { status: 204 });
});
