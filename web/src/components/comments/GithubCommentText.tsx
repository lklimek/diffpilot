import { commentText } from "../../lib/commentText";
import { CodeText } from "../section/CodeText";

/** Restricted display of a GitHub comment body; the source used for editing stays unchanged. */
export function GithubCommentText({ text }: { text: string }) {
  return <CodeText text={commentText(text)} linkPaths={false} />;
}
