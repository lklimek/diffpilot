import { expect, it } from "vitest";
import { foldKind, foldRuns } from "./fold";

it("folds consecutive hidden items and keeps the order", () => {
  expect(foldRuns([1, 2, 3, 4, 5, 6], (n) => n === 2 || n === 3 || n === 6)).toEqual([
    { kind: "item", item: 1 },
    { kind: "fold", items: [2, 3] },
    { kind: "item", item: 4 },
    { kind: "item", item: 5 },
    { kind: "fold", items: [6] },
  ]);
});

it.each([
  [["// a", "/* b */"], "comment"],
  [["use std::fmt;", "pub use a::b;", "import x from 'y';", "#include <a.h>"], "import"],
  [["use a;", "// b"], "comment and import"],
])("names the kind of %j", (texts, kind) => {
  expect(foldKind(texts)).toBe(kind);
});
