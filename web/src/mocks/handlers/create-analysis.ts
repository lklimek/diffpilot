import { HttpResponse } from "msw";
import { seedNow } from "../fixtures";
import { route } from "./common";

/** `POST /api/v1/analyses`: the fixture analysis, already finished (no progress in the mocks). */
export const createAnalysis = route("createAnalysis", () =>
  HttpResponse.json(seedNow("analysis") as object, { status: 201 }),
);
