import { HttpResponse } from "msw";
import type { MeResponse } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { scenario } from "../scenarios";
import { route } from "./common";

/** `GET /api/v1/me`: the signed-in fixture user with its CSRF token (scenario `signed-out`: nobody). */
export const getMe = route("getMe", () =>
  HttpResponse.json(scenario().me?.() ?? (seedNow("me") as MeResponse)),
);
