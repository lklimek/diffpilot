// Wire shapes the web app needs before crates/contracts has them (plan docs/review-workflow-plan.md RC7, T-L4).
// TODO(WP13-A CCR): replace with the generated types once the Ask subject kind `comment_item` is in the contracts;
// until then the msw mocks accept it and a real server answers 400 for it.
import type { AskAnchor, CommentItemId } from "./generated/types";

/** Ask subject "a review comment item" (plan §3.2 item 9): the chip reads "Comment C<no>". */
export interface CommentItemAnchor {
  kind: "comment_item";
  /** The item. */
  item: CommentItemId;
  /** Its per-PR number, `C<no>`. */
  no: number;
}

/** What an Ask question is about: a contract anchor or a pending one. */
export type ChatAnchor = AskAnchor | CommentItemAnchor;
