import { expect, it } from "vitest";
import type { Annotation } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { appendDraft, answerComment, flagComment, LLM_SUGGESTED } from "./prefill";

const flag: Annotation = {
  id: "a",
  type: "flag",
  basis: "inference",
  range: { start_line: 96, end_line: 99 },
  was: "",
  is: "",
  text: "Potential double send:",
  evidence: [],
  flag: {
    level: "red",
    scenario: ["The first reply is late.", "The client retries."],
    not_checked: "Not reproduced; no test was run.",
  },
};

it("a flag becomes plain text: the marker line, its title, the scenario numbered, what was not checked", () => {
  expect(LLM_SUGGESTED).toBe("LLM-suggested — review before posting");
  const text = flagComment(flag);
  expect(text).toBe(
    [
      "LLM-suggested — review before posting",
      "",
      "Potential double send:",
      "1. The first reply is late.",
      "2. The client retries.",
      "",
      "Not checked: Not reproduced; no test was run.",
    ].join("\n"),
  );
  expectTrustWording(text);
});

it("a flag without scenario or not-checked line keeps only its title", () => {
  expect(flagComment({ ...flag, flag: undefined })).toBe(`${LLM_SUGGESTED}\n\nPotential double send:`);
});

it("an Ask answer is taken as it is, its lists kept", () => {
  expect(answerComment("Two cases:\n- late reply\n- lost reply\n")).toBe(
    `${LLM_SUGGESTED}\n\nTwo cases:\n- late reply\n- lost reply`,
  );
});

it("text is appended to an existing draft after a blank line, never over it", () => {
  expect(appendDraft("", "B")).toBe("B");
  expect(appendDraft("  \n", "B")).toBe("B");
  expect(appendDraft("My note\n", "B")).toBe("My note\n\nB");
});
