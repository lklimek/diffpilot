import { route, seedJson } from "./common";

/** `GET /api/v1/me`: the signed-in fixture user with its CSRF token. */
export const getMe = route("getMe", seedJson("me"));
