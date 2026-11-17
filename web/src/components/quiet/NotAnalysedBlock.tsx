import { RuleLabel } from "./RuleLabel";
import { useState, type ReactNode } from "react";
import { Tooltip } from "../tooltip/Tooltip";
import type { LineRange, Counters, NotAnalysedItem, NotAnalysedSection } from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { dependencyFacts, rangeText } from "../section/format";
import { NOT_ANALYSED_LABEL, plural, testLines } from "./labels";
import { Note, QuietCard } from "./QuietCard";

interface NotAnalysedBlockProps {
  /** The section. */
  section: NotAnalysedSection;
  /** Deterministic counters of the pre-pass (`PrContextFacts.counters`). */
  counters: Counters;
  /** Code of an explicitly opened range. */
  code?: (item: NotAnalysedItem, range: LineRange) => ReactNode;
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
    return <p>{testLines(counters)}</p>;
  }
  if (section.category === "generated_mechanical") {
    return <p>{dependencyFacts(counters).join(". ")}. Dependency changes are counted, not interpreted.</p>;
  }
  return null;
}

/** A Not analysed section: what a built-in rule recognised, with the deterministic counters. */
export function NotAnalysedBlock({ section, counters, code }: NotAnalysedBlockProps) {
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
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
            {item.rule_kind && (
              <>
                {" "}
                · <RuleLabel kind={item.rule_kind} />
              </>
            )}
            {code &&
              (item.renderable ?? section.category !== "generated_mechanical") &&
              item.badge !== "binary_content" &&
              item.ranges?.map((range) => {
                const key = `${item.side}:${item.file}:${range.start_line}:${range.end_line}`;
                return (
                  <div key={key} className="min-w-0 my-2">
                    <Tooltip tip="Shows these lines with comment markers; click a line number to comment.">
                      <button
                        type="button"
                        aria-expanded={opened.has(key)}
                        className="cursor-pointer text-accent underline"
                        onClick={() =>
                          setOpened((all) => {
                            const next = new Set(all);
                            if (!next.delete(key)) next.add(key);
                            return next;
                          })
                        }
                      >
                        {opened.has(key) ? "Hide" : "Show"} lines {rangeText(range)}
                      </button>
                    </Tooltip>
                    {opened.has(key) && code(item, range)}
                  </div>
                );
              })}
          </li>
        ))}
      </ul>
    </QuietCard>
  );
}
