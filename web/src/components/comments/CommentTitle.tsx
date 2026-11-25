import type { CommentTitleFields } from "../../api/provisional/commentTitle";
import { ProvenanceTag } from "../section/ProvenanceTag";

export function CommentTitle({
  item,
  tip = true,
}: {
  item: CommentTitleFields & { title: string };
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

export function BotHeading({ item }: { item: CommentTitleFields & { title: string } }) {
  return item.llm_title ? (
    <span className="block text-xs font-normal text-muted">Bot heading: {item.title}</span>
  ) : null;
}
