import type { CommentItem } from "../../api/generated/types";
import { ProvenanceTag } from "../section/ProvenanceTag";

export function CommentTitle({
  item,
  tip = true,
}: {
  item: Pick<CommentItem, "llm_title"> & { title: string };
  tip?: boolean;
}) {
  return item.llm_title ? (
    <>
      {item.llm_title} <ProvenanceTag source="llm" tip={tip} />
    </>
  ) : (
    item.title
  );
}

export function BotHeading({ item }: { item: Pick<CommentItem, "llm_title"> & { title: string } }) {
  return item.llm_title ? (
    <span className="block text-xs font-normal text-muted">Bot heading: {item.title}</span>
  ) : null;
}
