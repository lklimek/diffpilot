import { HttpResponse } from "msw";
import { mockDependencies } from "../dependencies";
import { route, forAnalysis } from "./common";

/** `GET /api/v1/analyses/{id}/dependencies` (R15): the Cargo map, facts and crate notes. */
export const getDependencies = route(
  "getDependencies",
  forAnalysis(() => HttpResponse.json(mockDependencies())),
);
