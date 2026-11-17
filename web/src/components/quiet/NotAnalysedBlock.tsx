import type { Counters, NotAnalysedItem, NotAnalysedSection } from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { dependencyFacts, rangeText } from "../section/format";
import { NOT_ANALYSED_LABEL, plural } from "./labels";
import { Note, QuietCard } from "./QuietCard";

interface NotAnalysedBlockProps {
  /** The section. */
  section: NotAnalysedSection;
  /** Deterministic counters of the pre-pass (`PrContextFacts.counters`). */
  counters: Counters;
}

function linesText(item: NotAnalysedItem): string {
  if (item.ranges === null) return item.side === "base" ? "whole file, removed (base commit)" : "whole file";
  const ranges = item.ranges.map(rangeText).join(", ");
  return item.side === "base" ? `removed lines ${ranges} (base commit)` : `lines ${ranges}`;
}

function recognisedBy(section: NotAnalysedSection): string {
  const has = (badge: NotAnalysedItem["badge"]) => section.items.some((i) => i.badge === badge);
  const how = [
    has("classified_by_path") && "file path",
    has("classified_by_line_rule") && "line rule",
    has("binary_content") && "git's binary check",
  ].filter(Boolean);
  return `Recognised by ${how.length > 0 ? how.join(" and ") : "file path"}, no LLM.`;
}

function Counter({ section, counters }: NotAnalysedBlockProps) {
  if (section.category === "tests") {
    return <p>{plural(counters.lines_removed_from_tests, "line")} removed from tests</p>;
  }
  if (section.category === "generated_mechanical") {
    return <p>{dependencyFacts(counters).join(". ")}. Dependency changes are counted, not interpreted.</p>;
  }
  return null;
}

/** A Not analysed section: what a built-in rule recognised, with the deterministic counters. */
export function NotAnalysedBlock({ section, counters }: NotAnalysedBlockProps) {
  const badges = [...new Set(section.items.map((i) => i.badge))];
  return (
    <QuietCard
      title={
        <>
          {NOT_ANALYSED_LABEL[section.category]} · {plural(section.file_count, "file")}
          {badges.map((b) => (
            <ProvenanceBadge key={b} badge={b} />
          ))}
        </>
      }
    >
      <p>{recognisedBy(section)} The LLM did not analyse this material; the agent may read it as evidence.</p>
      <Counter section={section} counters={counters} />
      <Note>Changes per file, with the rule that matched:</Note>
      <ul className="m-0 list-disc pl-5 text-[12.5px]">
        {section.items.map((item) => (
          <li key={`${item.side}:${item.file}:${item.rule}`}>
            <code>{item.file}</code> <span className="text-muted">{linesText(item)}</span> · rule{" "}
            <code>{item.rule}</code>
          </li>
        ))}
      </ul>
    </QuietCard>
  );
}
