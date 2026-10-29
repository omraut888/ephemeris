import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App, DEFAULT_VIEW } from "../src/App";
import { AffordQuery } from "../src/components/AffordQuery";
import { ConeChart, bandIndex } from "../src/components/ConeChart";
import { Risk } from "../src/components/Risk";
import { StatePanel } from "../src/components/StatePanel";
import { money, percent } from "../src/format";
import { directEngine } from "../src/wasm/client";
import * as engine from "../src/wasm/engine";
import { FEED_JSON, REPORT, LOW_REPORT, reportWithoutIncome } from "./fixtures";

describe("StatePanel", () => {
  it("shows the estimate, the schedule and the named bills", () => {
    render(<StatePanel report={REPORT} />);
    const panel = screen.getByRole("complementary", { name: "Current state" });
    expect(within(panel).getByText(money(REPORT.balance.mean))).toBeTruthy();
    expect(
      within(panel).getByText(
        `±2σ ${money(REPORT.balance.low_2sd)} to ${money(REPORT.balance.high_2sd)}`,
      ),
    ).toBeTruthy();
    expect(within(panel).getByText("Payroll")).toBeTruthy();
    for (const name of ["Rent", "Streaming", "Phone", "Gym"]) {
      expect(within(panel).getByText(name)).toBeTruthy();
    }
  });

  it("lists recurring inflows apart from outflows, signed", () => {
    const interest = { ...REPORT.bills[0]!, stream: 99, name: "Interest payment", amount: 0.12 };
    render(<StatePanel report={{ ...REPORT, bills: [...REPORT.bills, interest] }} />);
    expect(screen.getByText("Recurring")).toBeTruthy();
    const ins = screen.getByRole("list", { name: "Recurring inflows" });
    expect(within(ins).getByText("Interest payment")).toBeTruthy();
    expect(within(ins).getByText("+$0.12")).toBeTruthy();
    const outs = screen.getByRole("list", { name: "Recurring outflows" });
    expect(within(outs).queryByText("Interest payment")).toBeNull();
    expect(within(outs).getByText("-$1,500.00")).toBeTruthy();
  });

  it("says so when there is no income", () => {
    render(<StatePanel report={reportWithoutIncome()} />);
    expect(screen.getByText(/No regular income detected/)).toBeTruthy();
  });
});

describe("Risk", () => {
  it("headlines the engine's P(below $0 on some day) over the horizon", () => {
    render(<Risk report={LOW_REPORT} />);
    const p = LOW_REPORT.forecast.targets[0]!.below_by.at(-1)!;
    expect(p).toBeGreaterThan(0.05);
    const risk = screen.getByTestId("risk");
    expect(risk.textContent).toContain(percent(p));
    expect(risk.textContent).toContain("below $0 at any point through");
  });

  it("renders nothing for a report without the $0 target", () => {
    const { container } = render(
      <Risk report={{ ...REPORT, forecast: { ...REPORT.forecast, targets: [] } }} />,
    );
    expect(container.textContent).toBe("");
  });
});

describe("ConeChart", () => {
  it("draws the $0 line in both views when $0 is in range", () => {
    for (const view of ["raw", "lows"] as const) {
      const { unmount } = render(<ConeChart report={LOW_REPORT} view={view} />);
      expect(screen.getByTestId("zero-line")).toBeTruthy();
      unmount();
    }
    render(<ConeChart report={REPORT} view="raw" />);
    expect(screen.queryByTestId("zero-line")).toBeNull();
  });

  it("picks the bands from the report's levels", () => {
    expect(bandIndex([0.05, 0.25, 0.5, 0.75, 0.95])).toEqual({ lo: 0, qlo: 1, mid: 2, qhi: 3, hi: 4 });
    // 0.9 is nearer 0.75 than 0.5 is.
    expect(bandIndex([0.1, 0.5, 0.9])).toEqual({ lo: 0, qlo: 0, mid: 1, qhi: 2, hi: 2 });
  });

  it("draws the raw daily cone", () => {
    render(<ConeChart report={REPORT} view="raw" />);
    const svg = screen.getByTestId("cone");
    expect(svg.getAttribute("data-view")).toBe("raw");
    const bands = screen.getByTestId("raw-bands");
    expect(bands.querySelectorAll("path")).toHaveLength(3);
    const last = REPORT.forecast.days.at(-1)!;
    expect(svg.getAttribute("aria-label")).toContain(`median ${money(last.quantiles[2]!)}`);
  });

  it("draws one stepped band per pay cycle, partial ones marked", () => {
    render(<ConeChart report={REPORT} view="lows" />);
    const cycles = REPORT.forecast.cycle_lows!.cycles;
    const drawn = screen.getAllByTestId("cycle");
    expect(drawn).toHaveLength(cycles.length);
    cycles.forEach((c, i) => {
      expect(drawn[i]!.classList.contains("partial")).toBe(c.partial);
    });
    expect(screen.queryByTestId("raw-bands")).toBeNull();
  });

  it("falls back to weekly cycles without income", () => {
    const r = reportWithoutIncome();
    expect(r.forecast.cycle_lows?.basis).toBe("week");
    render(<ConeChart report={r} view="lows" />);
    expect(screen.getAllByTestId("cycle")).toHaveLength(r.forecast.cycle_lows!.cycles.length);
  });

  it("shows the hovered day's quantiles", () => {
    render(<ConeChart report={REPORT} view="raw" />);
    const svg = screen.getByTestId("cone");
    svg.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 960, height: 340, right: 960, bottom: 340, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    fireEvent.pointerMove(svg, { clientX: 900, clientY: 100 });
    const tip = screen.getByRole("status");
    expect(tip.textContent).toContain("95%");
  });
});

describe("AffordQuery", () => {
  it("re-runs the forecast with the target and reads day D", async () => {
    const run = vi.fn(directEngine.forecast);
    const options = { horizon_days: 60, paths: 400, seed: 0 };
    render(<AffordQuery feedJson={FEED_JSON} options={options} report={REPORT} run={run} />);
    const date = REPORT.forecast.days[19]!.date;
    const user = userEvent.setup();
    const amount = screen.getByLabelText("Amount");
    await user.clear(amount);
    await user.type(amount, "3000");
    fireEvent.change(screen.getByLabelText("Date"), { target: { value: date } });
    await user.click(screen.getByRole("button", { name: "Check" }));
    const answer = await screen.findByTestId("afford-answer");

    expect(run).toHaveBeenCalledWith(FEED_JSON, { ...options, targets: [3000] });
    // The same numbers the engine reports for that target and day.
    const expected = engine.forecast(FEED_JSON, { ...options, targets: [3000] }).forecast.targets[0]!;
    expect(answer.textContent).toContain(percent(expected.at_least_on_day[19]!));
    expect(answer.textContent).toContain(percent(expected.reached_by[19]!));
  });

  it("explains a date outside the forecast", async () => {
    render(
      <AffordQuery feedJson={FEED_JSON} options={{ horizon_days: 60, paths: 400 }} report={REPORT} run={directEngine.forecast} />,
    );
    const date = screen.getByLabelText("Date");
    fireEvent.change(date, { target: { value: "1999-01-01" } });
    // The input's min/max stop a normal submit (as in a browser); submit
    // the form directly to reach the component's own guard.
    fireEvent.submit(date.closest("form")!);
    expect((await screen.findByRole("alert")).textContent).toMatch(/Pick a date/);
  });
});

describe("App", () => {
  it("opens on a demo, toggles the view and changes the horizon", async () => {
    render(<App engine={directEngine} />);
    await screen.findByTestId("cone");
    expect(screen.getByTestId("cone").getAttribute("data-view")).toBe(DEFAULT_VIEW);
    expect(DEFAULT_VIEW).toBe("raw");
    // The headline risk is shown above the chart in both views.
    expect(screen.getByTestId("risk").textContent).toMatch(/below \$0 at any point/);
    await waitFor(() =>
      expect(screen.getByTestId("source").textContent).toContain("demo · realistic · seed 7"),
    );

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Pay-cycle lows" }));
    expect(screen.getByTestId("cone").getAttribute("data-view")).toBe("lows");
    expect(screen.getByTestId("risk")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Daily" }));
    expect(screen.getByTestId("cone").getAttribute("data-view")).toBe("raw");

    await user.selectOptions(screen.getByLabelText("Horizon"), "30");
    expect(await screen.findByText(/Forecast · 30 days/)).toBeTruthy();
  });

  it("loads a feed file and reports a bad one", async () => {
    render(<App engine={directEngine} />);
    await screen.findByTestId("cone");
    const input = screen.getByTestId("file-input") as HTMLInputElement;
    const good = new File([FEED_JSON], "sandbox-feed.json", { type: "application/json" });
    await act(async () => {
      fireEvent.change(input, { target: { files: [good] } });
    });
    await waitFor(() => expect(screen.getByTestId("source").textContent).toContain("file · sandbox-feed.json"));

    const bad = new File(["{not json"], "broken.json", { type: "application/json" });
    await act(async () => {
      fireEvent.change(input, { target: { files: [bad] } });
    });
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("broken.json is not JSON"));

    const wrong = new File([JSON.stringify({ schema_version: 9 })], "v9.json");
    await act(async () => {
      fireEvent.change(input, { target: { files: [wrong] } });
    });
    await waitFor(() => expect(screen.getByRole("alert").textContent).toMatch(/^feed:/));
  });

  it("generates a new demo with the chosen mode and seed", async () => {
    render(<App engine={directEngine} />);
    await screen.findByTestId("cone");
    const user = userEvent.setup();
    await user.selectOptions(screen.getByLabelText("Demo mode"), "matched");
    const seed = screen.getByLabelText("Demo seed");
    await user.clear(seed);
    await user.type(seed, "12");
    await user.click(screen.getByRole("button", { name: "Demo" }));
    await waitFor(() =>
      expect(screen.getByTestId("source").textContent).toContain("demo · matched · seed 12"),
    );
  });
});
