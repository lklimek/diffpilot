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

it("the same text is not appended again when the draft already ends with it", () => {
  const once = appendDraft("My note", "B");
  expect(appendDraft(once, "B")).toBe(once);
  expect(appendDraft("B\n", "B")).toBe("B\n");
  expect(appendDraft("AB", "B")).toBe("AB\n\nB");
});

it("LLM text cannot mention, cross-link or make headings on GitHub: such tokens become code", () => {
  expect(answerComment("Needs @types/node and @Override; see #482 and acme/node-client#9.")).toBe(
    `${LLM_SUGGESTED}\n\nNeeds \`@types/node\` and \`@Override\`; see \`#482\` and \`acme/node-client#9\`.`,
  );
  expect(answerComment("# Heading\n## Sub\n#482 is the PR\nmail alice@example.com")).toBe(
    `${LLM_SUGGESTED}\n\n\\# Heading\n\\## Sub\n\`#482\` is the PR\nmail alice@example.com`,
  );
  // Code spans and fenced blocks stay as written.
  expect(answerComment("Use `@Override` or `#1`.\n```java\n@Override\n#include x\n```\nthen @bob")).toBe(
    `${LLM_SUGGESTED}\n\nUse \`@Override\` or \`#1\`.\n\`\`\`java\n@Override\n#include x\n\`\`\`\nthen \`@bob\``,
  );
  expect(
    flagComment({ ...flag, text: "# Race with @alice", flag: { ...flag.flag!, scenario: ["See #7."] } }),
  ).toBe(
    `${LLM_SUGGESTED}\n\n\\# Race with \`@alice\`\n1. See \`#7\`.\n\nNot checked: Not reproduced; no test was run.`,
  );
  // A URL keeps its @ and #.
  expect(answerComment("https://x.test/@u/p#12")).toBe(`${LLM_SUGGESTED}\n\nhttps://x.test/@u/p#12`);
});
