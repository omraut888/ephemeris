// The forecast cone, hand-drawn in SVG (docs/phase2-design.md §4). Two views
// of the same report:
// - raw: the daily 5–95% and 25–75% bands and the median;
// - lows: per pay cycle, the distribution of each path's lowest balance,
//   drawn flat across the cycle.
// Everything plotted is a number from the report; this file only maps
// values and day indices to pixels.

import { useMemo, useRef, useState } from "react";
import type { PointerEvent } from "react";

import { longDate, money, moneyAxis, shortDate } from "../format";
import type { ForecastReport } from "../schema.gen";

export type ConeView = "raw" | "lows";

const W = 960;
const H = 340;
const PAD = { left: 76, right: 16, top: 14, bottom: 28 };
const PLOT_W = W - PAD.left - PAD.right;
const PLOT_H = H - PAD.top - PAD.bottom;

/** Which entries of `levels` are the outer band, inner band and median. */
export interface BandIndex {
  lo: number;
  qlo: number;
  mid: number;
  qhi: number;
  hi: number;
}

export function bandIndex(levels: number[]): BandIndex {
  const nearest = (target: number) =>
    levels.reduce(
      (best, l, i) =>
        Math.abs(l - target) < Math.abs((levels[best] ?? 0) - target) ? i : best,
      0,
    );
  return {
    lo: 0,
    qlo: nearest(0.25),
    mid: nearest(0.5),
    qhi: nearest(0.75),
    hi: levels.length - 1,
  };
}

/** A round step for about `target` ticks over `span`. */
function tickStep(span: number, target: number): number {
  const raw = span / target;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const nice = norm < 1.5 ? 1 : norm < 3.5 ? 2 : norm < 7.5 ? 5 : 10;
  return nice * mag;
}

function levelLabel(l: number): string {
  return `${Math.round(l * 100)}%`;
}

interface Props {
  report: ForecastReport;
  view: ConeView;
}

export function ConeChart({ report, view }: Props) {
  const { days, levels } = report.forecast;
  const cycles = report.forecast.cycle_lows?.cycles ?? [];
  const idx = bandIndex(levels);
  const n = days.length;
  const svgRef = useRef<SVGSVGElement>(null);
  const [hover, setHover] = useState<number | null>(null);

  // Day index of each date, so a cycle can be placed on the day axis.
  const dayIndex = useMemo(() => new Map(days.map((d, i) => [d.date, i])), [days]);

  // Value range of what this view draws, plus today's ±2σ marker.
  const [yMin, yMax] = useMemo(() => {
    const values: number[] = [report.balance.low_2sd, report.balance.high_2sd];
    if (view === "raw") {
      for (const d of days) {
        values.push(d.quantiles[idx.lo] ?? 0, d.quantiles[idx.hi] ?? 0);
      }
    } else {
      for (const c of cycles) {
        values.push(c.quantiles[idx.lo] ?? 0, c.quantiles[idx.hi] ?? 0);
      }
    }
    const lo = Math.min(...values);
    const hi = Math.max(...values);
    const pad = (hi - lo) * 0.06 || 1;
    return [lo - pad, hi + pad];
  }, [days, cycles, view, idx.lo, idx.hi, report.balance]);

  // Day i is the slot ending at x(i); today (as_of) is x(-1), the left edge.
  const x = (i: number) => PAD.left + ((i + 1) / n) * PLOT_W;
  const y = (v: number) => PAD.top + ((yMax - v) / (yMax - yMin)) * PLOT_H;

  const step = tickStep(yMax - yMin, 5);
  const yTicks: number[] = [];
  for (let t = Math.ceil(yMin / step) * step; t <= yMax; t += step) yTicks.push(t);
  const xTicks = days
    .map((d, i) => ({ d, i }))
    .filter(({ i }) => i % Math.max(1, Math.round(n / 6)) === 0);

  const area = (upper: number, lower: number) => {
    const top = days.map((d, i) => `${x(i)},${y(d.quantiles[upper] ?? 0)}`);
    const bottom = days
      .map((d, i) => `${x(i)},${y(d.quantiles[lower] ?? 0)}`)
      .reverse();
    return `M${top.join("L")}L${bottom.join("L")}Z`;
  };
  const medianLine = `M${days
    .map((d, i) => `${x(i)},${y(d.quantiles[idx.mid] ?? 0)}`)
    .join("L")}`;

  const cycleSpans = cycles.map((c) => ({
    c,
    x0: x((dayIndex.get(c.start) ?? 0) - 1),
    x1: x(dayIndex.get(c.end) ?? n - 1),
  }));

  const last = days[n - 1];
  const summary = last
    ? `Forecast to ${longDate(last.date)}: median ${money(
        last.quantiles[idx.mid] ?? 0,
      )}, 90% between ${money(last.quantiles[idx.lo] ?? 0)} and ${money(
        last.quantiles[idx.hi] ?? 0,
      )}.`
    : "Empty forecast.";

  const onMove = (e: PointerEvent<SVGSVGElement>) => {
    const rect = svgRef.current?.getBoundingClientRect();
    if (!rect || rect.width === 0) return;
    const px = ((e.clientX - rect.left) / rect.width) * W;
    const i = Math.round(((px - PAD.left) / PLOT_W) * n - 1);
    setHover(Math.min(n - 1, Math.max(0, i)));
  };

  const hoverDay = hover === null ? undefined : days[hover];
  const hoverCycle =
    hoverDay && view === "lows"
      ? cycles.find((c) => c.start <= hoverDay.date && hoverDay.date <= c.end)
      : undefined;
  const crossesZero = yMin < 0 && yMax > 0;

  return (
    <div className="chart">
      <svg
        ref={svgRef}
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label={summary}
        data-testid="cone"
        data-view={view}
        onPointerMove={onMove}
        onPointerLeave={() => setHover(null)}
      >
        <g className="grid axis">
          {yTicks.map((t) => (
            <g key={t}>
              <line x1={PAD.left} x2={W - PAD.right} y1={y(t)} y2={y(t)} />
              <text x={PAD.left - 8} y={y(t) + 4} textAnchor="end">
                {moneyAxis(t)}
              </text>
            </g>
          ))}
          {xTicks.map(({ d, i }) => (
            <text key={d.date} x={x(i)} y={H - 8} textAnchor="middle">
              {shortDate(d.date)}
            </text>
          ))}
        </g>
        {crossesZero && (
          <g className="zero" data-testid="zero-line">
            <line x1={PAD.left} x2={W - PAD.right} y1={y(0)} y2={y(0)} />
            <text x={W - PAD.right - 4} y={y(0) - 5} textAnchor="end">
              $0
            </text>
          </g>
        )}
        <line className="today" x1={x(-1)} x2={x(-1)} y1={PAD.top} y2={H - PAD.bottom} />

        {view === "raw" ? (
          <g data-testid="raw-bands">
            <path className="band-outer" d={area(idx.hi, idx.lo)} />
            <path className="band-inner" d={area(idx.qhi, idx.qlo)} />
            <path className="median" d={medianLine} />
          </g>
        ) : (
          <g data-testid="cycle-bands">
            {cycleSpans.map(({ c, x0, x1 }) => (
              <g key={c.start} className={c.partial ? "partial" : undefined} data-testid="cycle">
                <rect
                  className="band-outer"
                  x={x0}
                  width={x1 - x0}
                  y={y(c.quantiles[idx.hi] ?? 0)}
                  height={y(c.quantiles[idx.lo] ?? 0) - y(c.quantiles[idx.hi] ?? 0)}
                />
                <rect
                  className="band-inner"
                  x={x0}
                  width={x1 - x0}
                  y={y(c.quantiles[idx.qhi] ?? 0)}
                  height={y(c.quantiles[idx.qlo] ?? 0) - y(c.quantiles[idx.qhi] ?? 0)}
                />
                <line
                  className="median"
                  x1={x0}
                  x2={x1}
                  y1={y(c.quantiles[idx.mid] ?? 0)}
                  y2={y(c.quantiles[idx.mid] ?? 0)}
                />
              </g>
            ))}
          </g>
        )}

        {/* Today: the estimated balance with its ±2σ range. */}
        <line
          className="now-marker"
          x1={x(-1)}
          x2={x(-1)}
          y1={y(report.balance.low_2sd)}
          y2={y(report.balance.high_2sd)}
        >
          <title>{`Today ${money(report.balance.mean)} ±2σ`}</title>
        </line>
        <circle cx={x(-1)} cy={y(report.balance.mean)} r={3.5} fill="var(--accent)" />

        {hover !== null && (
          <line className="hover-line" x1={x(hover)} x2={x(hover)} y1={PAD.top} y2={H - PAD.bottom} />
        )}
      </svg>

      {hoverDay && (
        <div
          className="tooltip"
          role="status"
          style={{
            left: `${Math.min(78, (x(hover ?? 0) / W) * 100)}%`,
            top: 12,
          }}
        >
          {view === "lows" && hoverCycle ? (
            <>
              <div className="label">
                Lowest, {shortDate(hoverCycle.start)} – {shortDate(hoverCycle.end)}
                {hoverCycle.partial ? " (partial)" : ""}
              </div>
              {levels.map((l, k) => (
                <div className="row" key={l}>
                  <span className="dim">{levelLabel(l)}</span>
                  <span className="num">{money(hoverCycle.quantiles[k] ?? 0)}</span>
                </div>
              ))}
            </>
          ) : (
            <>
              <div className="label">{longDate(hoverDay.date)}</div>
              {levels.map((l, k) => (
                <div className="row" key={l}>
                  <span className="dim">{levelLabel(l)}</span>
                  <span className="num">{money(hoverDay.quantiles[k] ?? 0)}</span>
                </div>
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}
