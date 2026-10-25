// Formatting only: turning numbers and ISO date strings the engine produced
// into text. No arithmetic on forecast data happens here.

import type { IsoDate, PayCadence } from "./schema.gen";

const wholeDollars = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  maximumFractionDigits: 0,
});
const cents = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const compact = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  notation: "compact",
  maximumFractionDigits: 1,
});

/** `$1,234` (or `$1,234.56` with `withCents`); `-$48` for negatives. */
export function money(x: number, withCents = false): string {
  const s = (withCents ? cents : wholeDollars).format(x);
  // Intl renders −0 as "-$0"; a balance of zero has no sign.
  return s === "-$0" || s === "-$0.00" ? s.slice(1) : s;
}

/** `+$31` or `-$97`: a change, always signed. */
export function signedMoney(x: number): string {
  const s = money(Math.abs(x));
  return x < 0 ? `-${s}` : `+${s}`;
}

/** `$4.6K`, for axis labels. */
export function moneyCompact(x: number): string {
  return compact.format(x);
}

/** `61%`; extremes as `<1%` and `>99%` so 0 and 1 never look certain. */
export function percent(p: number): string {
  if (p <= 0) return "<1%";
  if (p >= 1) return ">99%";
  const s = Math.round(p * 100);
  if (s < 1) return "<1%";
  if (s > 99) return ">99%";
  return `${s}%`;
}

const MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/** `Oct 22` from `2026-10-22`. */
export function shortDate(d: IsoDate): string {
  const month = MONTHS[Number(d.slice(5, 7)) - 1] ?? "?";
  return `${month} ${Number(d.slice(8, 10))}`;
}

/** `Oct 22, 2026`. */
export function longDate(d: IsoDate): string {
  return `${shortDate(d)}, ${d.slice(0, 4)}`;
}

/** `1st`, `22nd`, `31st`. */
export function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  switch (n % 10) {
    case 1:
      return `${n}st`;
    case 2:
      return `${n}nd`;
    case 3:
      return `${n}rd`;
    default:
      return `${n}th`;
  }
}

export function cadence(c: PayCadence): string {
  switch (c.kind) {
    case "weekly":
      return "weekly";
    case "biweekly":
      return "every 2 weeks";
    case "semi_monthly":
      return `on the ${ordinal(c.days[0])} and ${ordinal(c.days[1])}`;
    case "monthly":
      return `monthly on the ${ordinal(c.day)}`;
  }
}
