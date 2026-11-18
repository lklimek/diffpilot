import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";
import { ANALYSE_AGAIN_SENTENCE } from "./again";

// The flow button warns that analysing again replaces the steps when the server's message ends with this sentence:
// a reworded server text must fail here, not silently drop the warning.
const source = (path: string) => readFileSync(join(process.cwd(), "..", path), "utf8");

it("the sentence the web matches is the one the server writes (rejected stored flow, cancelled re-analysis)", () => {
  expect(source("crates/server/src/pipeline/tour.rs")).toContain(`"${ANALYSE_AGAIN_SENTENCE}"`);
  expect(source("crates/db/src/repo/flow_work.rs")).toContain(` ${ANALYSE_AGAIN_SENTENCE}";`);
});
