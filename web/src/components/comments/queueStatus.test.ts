import { describe, expect, it } from "vitest";
import fixture from "../../../../docs/contracts/fixtures/needs-you-cases.json";
import type { CommentItem, QueueEntry } from "../../api/generated/types";
import { commentsByNo, commentStatus, needsYouQueue } from "./queueStatus";

const items = fixture.items.map((item) => ({ ...fixture.defaults, ...item }) as CommentItem);
const queue = fixture.queue as QueueEntry[];

describe("Needs you fixture", () => {
  it.each(items.map((item, index) => [item.no, item, fixture.expected_statuses[index]] as const))(
    "classifies C%i like the server",
    (_no, item, expected) => expect(commentStatus(item)).toBe(expected),
  );

  it("counts only queue members, including gates", () => {
    const byNo = commentsByNo(items)!;
    const numbers = needsYouQueue(queue, byNo).flatMap((entry) => entry.item_nos);
    expect(numbers).toHaveLength(fixture.expected_needs_you);
    expect(numbers.filter((no) => byNo.get(no)!.gates.length > 0)).toHaveLength(
      fixture.expected_needs_you_gates,
    );
  });
});
