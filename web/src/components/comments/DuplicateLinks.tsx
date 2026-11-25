import type { CommentItem } from "../../api/generated/types";
import { GithubMark } from "../github/GithubMark";
import { isGithubUrl } from "../github/links";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { itemId } from "./parts";

export function DuplicateLinks({ primary, duplicates }: { primary: CommentItem; duplicates: CommentItem[] }) {
  if (!duplicates.length) return null;
  return (
    <p className="my-1 text-xs">
      Also raised as{" "}
      {duplicates.map((item, i) => {
        const label = `${itemId(item)} (${item.author})`;
        return (
          <span key={item.id}>
            {i > 0 && ", "}
            {item.url && isGithubUrl(item.url) ? (
              <a href={item.url} target="_blank" rel="noopener noreferrer">
                {label}
                <GithubMark />
              </a>
            ) : (
              label
            )}
          </span>
        );
      })}
      . Deciding {itemId(primary)} decides them too. <ProvenanceTag source="llm" />
    </p>
  );
}
