// The mocks' source of file lines: a real scenario's synthetic files, else the fixture PR's seed.
import type { LineSide, LinesResponse } from "../api/generated/types";
import { fixtureState, linesFor } from "./fixtures";
import { realLines } from "./real";
import { realTour } from "./scenarios";

/** Lines `from..=to` of `path` on `side`; undefined when the file is not part of the analysis. */
export function mockLines(path: string, side: LineSide, from: number, to: number): LinesResponse | undefined {
  const tour = realTour();
  return tour ? realLines(tour, path, side, from, to) : linesFor(fixtureState(), path, side, from, to);
}
