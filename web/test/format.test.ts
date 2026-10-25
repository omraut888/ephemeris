// @vitest-environment node
import { describe, expect, it } from "vitest";

import { cadence, longDate, money, moneyCompact, ordinal, percent, shortDate, signedMoney } from "../src/format";

describe("format", () => {
  it("formats money", () => {
    expect(money(1234.56)).toBe("$1,235");
    expect(money(1234.56, true)).toBe("$1,234.56");
    expect(money(-48.2)).toBe("-$48");
    expect(money(-0.001, true)).toBe("$0.00");
    expect(signedMoney(31.2)).toBe("+$31");
    expect(signedMoney(-97.6)).toBe("-$98");
    expect(moneyCompact(4600)).toBe("$4.6K");
  });

  it("never prints a probability as certain", () => {
    expect(percent(0)).toBe("<1%");
    expect(percent(0.004)).toBe("<1%");
    expect(percent(0.61)).toBe("61%");
    expect(percent(0.996)).toBe(">99%");
    expect(percent(1)).toBe(">99%");
  });

  it("formats dates and cadences from the report's strings", () => {
    expect(shortDate("2026-10-02")).toBe("Oct 2");
    expect(longDate("2024-12-31")).toBe("Dec 31, 2024");
    expect([1, 2, 3, 4, 11, 12, 13, 21, 22, 31].map(ordinal).join(" ")).toBe(
      "1st 2nd 3rd 4th 11th 12th 13th 21st 22nd 31st",
    );
    expect(cadence({ kind: "semi_monthly", days: [15, 31] })).toBe("on the 15th and 31st");
    expect(cadence({ kind: "monthly", day: 1 })).toBe("monthly on the 1st");
    expect(cadence({ kind: "biweekly", anchor: "2024-01-05" })).toBe("every 2 weeks");
  });
});
