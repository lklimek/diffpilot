import type { RuleKind } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
const RULES: Record<RuleKind, [string, string]> = {
  path_glob: ["path glob", "Matches the file path."],
  line_regex: ["line regex", "Matches a line with a regular expression."],
  lexer: ["lexer", "Recognises comment-only lines with the Rust compiler lexer."],
  header: ["generated-file header", "Recognises a generated-file header."],
  binary: ["binary content", "Git identifies binary content."],
};
/** Names the detector that actually matched, rather than treating every rule as a regex. */
export function RuleLabel({ kind }: { kind: RuleKind }) {
  const [label, tip] = RULES[kind];
  return (
    <Tooltip tip={tip}>
      <span tabIndex={0} className="cursor-help">
        {label}
      </span>
    </Tooltip>
  );
}
