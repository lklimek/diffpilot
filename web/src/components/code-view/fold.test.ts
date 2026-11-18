import { expect, it } from "vitest";
import { foldRuns } from "./fold";

it("folds consecutive hidden items and keeps the order", () => {
  expect(foldRuns([1, 2, 3, 4, 5, 6], (n) => n === 2 || n === 3 || n === 6)).toEqual([
    { kind: "item", item: 1 },
    { kind: "fold", items: [2, 3] },
    { kind: "item", item: 4 },
    { kind: "item", item: 5 },
    { kind: "fold", items: [6] },
  ]);
});
