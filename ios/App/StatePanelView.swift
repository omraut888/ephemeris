// The current state: the estimate with its ranges, the pay schedule, the
// recurring streams in and out, and regime changes
// (web/src/components/StatePanel.tsx).

import EphemerisKit
import SwiftUI

struct StatePanelView: View {
    let report: ForecastReport

    var body: some View {
        Panel {
            SectionLabel("State · as of \(Format.longDate(report.asOf))")
            stat("Balance", report.balance)
            stat("Spend rate", report.spendRate, unit: "/day", range: report.spendRateRange)
            stat("Paycheck", report.expectedPaycheck)
            Text("Settled through \(Format.longDate(report.settledThrough)); later days still hold pending amounts.")
                .font(.caption)
                .foregroundStyle(Theme.dim)

            Divider().overlay(Theme.line)
            SectionLabel("Pay schedule")
            if let inc = report.income {
                row(inc.name ?? "Paycheck", Format.cadence(inc.cadence),
                    inc.nextScheduled.map { "next \(Format.shortDate($0))" } ?? "—")
            } else {
                Text("No regular income detected: the forecast has no paychecks.")
                    .font(.callout).foregroundStyle(Theme.dim)
            }

            Divider().overlay(Theme.line)
            SectionLabel("Recurring")
            if report.bills.isEmpty {
                Text("No recurring streams detected.").font(.callout).foregroundStyle(Theme.dim)
            } else {
                recurring("In", report.bills.filter { $0.amount > 0 })
                recurring("Out", report.bills.filter { $0.amount <= 0 })
            }

            if !report.regimeChanges.isEmpty {
                Divider().overlay(Theme.line)
                SectionLabel("Regime changes")
                ForEach(Array(report.regimeChanges.enumerated()), id: \.offset) { _, c in
                    row(
                        c.channel == .spend ? "Spend rate" : "Paycheck",
                        Format.longDate(c.date) + (c.settled ? "" : " · provisional"),
                        Format.signedMoney(c.shift) + (c.channel == .spend ? "/day" : "")
                    )
                }
                Text("The cone widens until the filter relearns the new level.")
                    .font(.caption).foregroundStyle(Theme.dim)
            }
        }
    }

    /// A value with its ±2σ, or the floored range when the forecast floors it.
    private func stat(_ label: String, _ e: Estimate, unit: String = "", range: SpendRange? = nil) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label).foregroundStyle(Theme.dim)
            Spacer()
            VStack(alignment: .trailing, spacing: 2) {
                (Text(Format.money(e.mean)) + Text(unit).foregroundStyle(Theme.dim))
                    .font(.system(.title3, design: .monospaced).weight(.semibold))
                Group {
                    if let range {
                        Text("range \(Format.money(range.low)) to \(Format.money(range.high))")
                        Text("floored at \(Format.money(range.floor))")
                    } else {
                        Text("±2σ \(Format.money(e.low2sd)) to \(Format.money(e.high2sd))")
                    }
                }
                .font(.caption.monospaced())
                .foregroundStyle(Theme.dim)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private func row(_ title: String, _ sub: String, _ value: String, color: Color = Theme.text) -> some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                Text(sub).font(.caption).foregroundStyle(Theme.dim)
            }
            Spacer()
            Text(value).font(.callout.monospaced()).foregroundStyle(color)
        }
    }

    /// Streams of one direction; the engine signs amounts (inflow +).
    @ViewBuilder private func recurring(_ title: String, _ bills: [BillReport]) -> some View {
        if !bills.isEmpty {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(Theme.dim)
            ForEach(bills, id: \.stream) { b in
                let cadence = b.monthlyOn.map { "monthly on the \(Format.ordinal($0.day))" } ?? "every \(b.periodDays) days"
                row(
                    b.name ?? "Stream \(b.stream)",
                    "\(cadence) · next \(Format.shortDate(b.nextDue))",
                    Format.signedMoney(b.amount),
                    color: b.amount > 0 ? Theme.good : Theme.text
                )
            }
        }
    }
}
