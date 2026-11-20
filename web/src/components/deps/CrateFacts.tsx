import type { ConversionFact, ConversionKind, CrateChanges } from "../../api/generated/types";
import { CheckBadge } from "../badge/Badge";
import { LineLinkTo } from "./LineLinkTo";
import { count, type CrateOrigin, type DepsLinks } from "./model";

const KIND: Record<ConversionKind, string> = {
  from_impl: "From impl",
  try_from_impl: "TryFrom impl",
  impl_on_foreign_type: "impl on its type",
  conversion_fn: "conversion function",
};

/** What the token rules cover (brief 2026-11-20: Facts); shown under every list of facts. */
export const DETECTOR_LIMITS =
  "Found by token rules for From/TryFrom impls, impls on another crate's type and parse_/validate_/… functions. Macros, glob imports and re-exports are not followed. No finding here is not evidence that none exist.";

const TOKENS_TIP =
  "Deterministic check: Rust token rules on the added lines at the head commit; the code was not compiled or run.";

function Definition({ fact, links }: { fact: ConversionFact; links: DepsLinks }) {
  const { definition } = fact;
  switch (definition.kind) {
    case "located":
      return (
        <>
          defined at <LineLinkTo target={{ ...definition.at, side: "head" }} links={links} />
        </>
      );
    case "not_located":
      return <>definition not found in {fact.type_crate} (generated, a macro or a re-export)</>;
    case "not_searched":
      return <>definition not searched</>;
    case "other_repository":
      return <>defined in another repository (not looked up)</>;
  }
}

function Fact({ fact, origin, links }: { fact: ConversionFact; origin?: CrateOrigin; links: DepsLinks }) {
  return (
    <li className="grid gap-0.5">
      <span className="flex flex-wrap items-baseline gap-x-1.5">
        <span className="font-semibold">{KIND[fact.kind]}</span> for <code>{fact.written}</code> from{" "}
        <code>{fact.type_crate}</code>
        {origin && ` in ${origin.repository}${origin.yours ? " (yours)" : ""}`}{" "}
        <CheckBadge text="Rust tokens" tip={TOKENS_TIP} />
      </span>
      <span className="flex flex-wrap items-baseline gap-x-1.5 text-[12.5px] text-muted">
        at <LineLinkTo target={{ ...fact.at, side: "head" }} links={links} /> ·
        {fact.imported_at ? (
          <>
            {" "}
            imported at <LineLinkTo target={{ ...fact.imported_at, side: "head" }} links={links} />
          </>
        ) : (
          " no use line (path written in full)"
        )}{" "}
        · <Definition fact={fact} links={links} />
      </span>
    </li>
  );
}

/** Conversions a changed crate adds for types of other crates: deterministic facts with lines, never a verdict. */
export function CrateFacts({
  changes,
  origins = {},
  links,
}: {
  changes: CrateChanges;
  /** Repositories of the map's crates by name (`crateOrigins`); a fact's crate not in it is named alone. */
  origins?: Readonly<Record<string, CrateOrigin>>;
  links: DepsLinks;
}) {
  return (
    <div className="grid gap-1.5">
      {changes.facts.length > 0 ? (
        <ul className="m-0 grid list-none gap-2 p-0">
          {changes.facts.map((fact) => (
            <Fact
              key={`${fact.at.file}:${fact.at.line}:${fact.kind}:${fact.written}`}
              fact={fact}
              origin={origins[fact.type_crate]}
              links={links}
            />
          ))}
        </ul>
      ) : (
        <p className="m-0">None found for types of this repository&apos;s or your crates.</p>
      )}
      {changes.facts_hidden > 0 && (
        <p className="m-0">
          {count(changes.facts_hidden, "more fact is", "more facts are")} not listed (limit 50).
        </p>
      )}
      {changes.other_facts > 0 && (
        <p className="m-0">
          {count(changes.other_facts, "conversion")} for types of other crates{" "}
          {changes.other_facts === 1 ? "is" : "are"} counted, not listed.
        </p>
      )}
      <p className="m-0 text-xs text-muted">{DETECTOR_LIMITS}</p>
    </div>
  );
}
