// Small hand-written SVG charts of the side panel (mockup `colChart`, `barChart`); colours are theme tokens.

interface MiniColumnChartProps {
  title: string;
  /** Right-aligned note of the title, e.g. `per week`. */
  note?: string;
  /** One column per label, oldest first. */
  values: number[];
  labels: string[];
  /** The last column is the running period (drawn dashed, labelled `now`). */
  lastPartial?: boolean;
}

const W = 300;

/** Columns per period; the largest and the last carry their value. */
export function MiniColumnChart({ title, note, values, labels, lastPartial = false }: MiniColumnChartProps) {
  const H = 74;
  const top = 14;
  const bottom = 16;
  const slot = W / Math.max(1, values.length);
  const bar = Math.min(18, slot * 0.6);
  const max = Math.max(1, ...values);
  return (
    <figure className="m-0">
      <figcaption className="mb-0.5 flex gap-2 text-xs font-semibold">
        {title}
        {note && <span className="ml-auto font-medium text-muted">{note}</span>}
      </figcaption>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label={`${title}: ${labels.map((label, i) => `${label} ${values[i]}`).join(", ")}${lastPartial ? " (last so far)" : ""}`}
        className="block h-auto w-full max-w-[380px]"
      >
        {values.map((value, i) => {
          const h = (value / max) * (H - top - bottom);
          const x = slot * i + slot / 2 - bar / 2;
          const yTop = H - bottom - h;
          const last = i === values.length - 1;
          const partial = last && lastPartial;
          return (
            <g key={i}>
              {/* The running period: a dashed outline, visible on both themes and at 0. */}
              <rect
                x={x}
                y={partial ? Math.min(yTop, H - bottom - 3) : yTop}
                width={bar}
                height={partial ? Math.max(h, 3) : Math.max(h, 1)}
                rx={3}
                data-partial={partial || undefined}
                className={partial ? "fill-none stroke-c-pos" : "fill-c-pos"}
                strokeWidth={partial ? 1.5 : undefined}
                strokeDasharray={partial ? "3 2" : undefined}
              >
                <title>{`${labels[i]}: ${value}`}</title>
              </rect>
              {(last || value === max) && (
                <text
                  x={x + bar / 2}
                  y={yTop - 3}
                  textAnchor="middle"
                  className="fill-text text-[11px] font-bold"
                >
                  {value}
                </text>
              )}
              {(i % 2 === 1 || last) && (
                <text x={x + bar / 2} y={H - 3} textAnchor="middle" className="fill-muted text-[11px]">
                  {last && lastPartial ? "now" : labels[i]}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </figure>
  );
}

interface MiniBarChartProps {
  title: string;
  /** One bar per label. */
  values: number[];
  labels: string[];
  /** Shared scale across charts; default the largest value. */
  max?: number;
}

/** Horizontal bars with their values (e.g. ages of waiting reviews). */
export function MiniBarChart({ title, values, labels, max }: MiniBarChartProps) {
  const row = 20;
  const labelWidth = 54;
  const scale = Math.max(1, max ?? Math.max(...values));
  return (
    <figure className="m-0">
      <figcaption className="mb-0.5 text-xs font-semibold">{title}</figcaption>
      <svg
        viewBox={`0 0 ${W} ${row * values.length}`}
        role="img"
        aria-label={`${title}: ${labels.map((label, i) => `${label} ${values[i]}`).join(", ")}`}
        className="block h-auto w-full max-w-[380px]"
      >
        {values.map((value, i) => {
          const w = (value / scale) * (W - labelWidth - 24);
          const y = i * row + 3;
          return (
            <g key={i}>
              <text x={0} y={y + 11} className="fill-muted text-[11px]">
                {labels[i]}
              </text>
              <rect
                x={labelWidth}
                y={y}
                width={Math.max(w, value ? 4 : 0)}
                height={13}
                rx={3}
                className="fill-c-pos"
              >
                <title>{`${labels[i]}: ${value}`}</title>
              </rect>
              <text x={labelWidth + w + 5} y={y + 11} className="fill-text text-[11px] font-bold">
                {value}
              </text>
            </g>
          );
        })}
      </svg>
    </figure>
  );
}
