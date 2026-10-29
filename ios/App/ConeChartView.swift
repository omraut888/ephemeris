// The cone in Swift Charts (docs/phase3-design.md §3), from the report's
// quantiles as they are: the Daily view's bands and median per day, or each
// pay cycle's lows as a flat step. Day i sits at x = i + 1; today (as_of) is
// x = 0, with the balance estimate and its ±2σ. Mapping values to the plot is
// presentation; no statistic is computed here.

import Charts
import EphemerisKit
import SwiftUI

struct ConeChartView: View {
    let report: ForecastReport
    let view: AppModel.ConeView
    @State private var selected: Int?

    private var days: [ConeDay] { report.forecast.days }
    private var cycles: [CycleLowReport] { report.forecast.cycleLows?.cycles ?? [] }
    private var band: BandIndex { BandIndex(levels: report.forecast.levels) }

    /// Each cycle's span on the day axis.
    private var spans: [(cycle: CycleLowReport, start: Int, end: Int)] {
        let index = Dictionary(days.enumerated().map { ($1.date, $0) }, uniquingKeysWith: { a, _ in a })
        return cycles.compactMap { c in
            guard let s = index[c.start], let e = index[c.end] else { return nil }
            return (c, s, e)
        }
    }

    /// What this view draws, plus today's ±2σ, padded 6% (as the web chart).
    private var yDomain: ClosedRange<Double> {
        var values = [report.balance.low2sd, report.balance.high2sd]
        let rows = view == .daily ? days.map(\.quantiles) : cycles.map(\.quantiles)
        for q in rows where q.count > band.hi {
            values += [q[band.lo], q[band.hi]]
        }
        let lo = values.min() ?? 0
        let hi = values.max() ?? 1
        let pad = (hi - lo) * 0.06
        return pad > 0 ? (lo - pad)...(hi + pad) : (lo - 1)...(hi + 1)
    }

    var body: some View {
        let domain = yDomain
        Chart {
            if domain.lowerBound < 0, domain.upperBound > 0 {
                RuleMark(y: .value("Balance", 0))
                    .foregroundStyle(Theme.warn)
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [4, 4]))
                    .annotation(position: .top, alignment: .trailing) {
                        Text("$0").font(.caption2.monospaced()).foregroundStyle(Theme.warn)
                    }
            }
            RuleMark(x: .value("Day", 0))
                .foregroundStyle(Theme.faint)
                .lineStyle(StrokeStyle(lineWidth: 1, dash: [2, 3]))

            if view == .daily {
                dailyMarks
            } else {
                lowsMarks
            }

            RuleMark(
                x: .value("Day", 0),
                yStart: .value("Balance", report.balance.low2sd),
                yEnd: .value("Balance", report.balance.high2sd)
            )
            .foregroundStyle(Theme.accent.opacity(0.5))
            .lineStyle(StrokeStyle(lineWidth: 3, lineCap: .round))
            PointMark(x: .value("Day", 0), y: .value("Balance", report.balance.mean))
                .foregroundStyle(Theme.accent)
                .symbolSize(30)

            if let s = selected {
                RuleMark(x: .value("Day", s)).foregroundStyle(Theme.dim.opacity(0.6))
            }
        }
        .chartXScale(domain: 0...max(days.count, 1))
        .chartYScale(domain: domain)
        .chartXSelection(value: $selected)
        .chartXAxis {
            AxisMarks(values: xTicks) { value in
                AxisGridLine().foregroundStyle(Theme.line)
                AxisValueLabel {
                    if let v = value.as(Int.self) {
                        Text(Format.shortDate(v == 0 ? report.asOf : days[v - 1].date))
                            .font(.caption2.monospaced())
                    }
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .leading, values: .automatic(desiredCount: 5)) { value in
                AxisGridLine().foregroundStyle(Theme.line)
                AxisValueLabel {
                    if let v = value.as(Double.self) {
                        Text(Format.moneyAxis(v)).font(.caption2.monospaced())
                    }
                }
            }
        }
        .frame(height: 280)
        .overlay(alignment: .top) {
            if let s = selected, s >= 1, s <= days.count {
                tooltip(day: s - 1).padding(.top, 2)
            }
        }
        .accessibilityIdentifier("cone")
        .accessibilityValue(view == .daily ? "daily" : "lows")
    }

    @ChartContentBuilder private var dailyMarks: some ChartContent {
        ForEach(Array(days.enumerated()), id: \.offset) { i, d in
            AreaMark(
                x: .value("Day", i + 1),
                yStart: .value("Balance", d.quantiles[band.lo]),
                yEnd: .value("Balance", d.quantiles[band.hi]),
                series: .value("Band", "outer")
            )
            .foregroundStyle(Theme.bandOuter)
            AreaMark(
                x: .value("Day", i + 1),
                yStart: .value("Balance", d.quantiles[band.qlo]),
                yEnd: .value("Balance", d.quantiles[band.qhi]),
                series: .value("Band", "inner")
            )
            .foregroundStyle(Theme.bandInner)
            LineMark(
                x: .value("Day", i + 1),
                y: .value("Balance", d.quantiles[band.mid]),
                series: .value("Band", "median")
            )
            .foregroundStyle(Theme.median)
            .lineStyle(StrokeStyle(lineWidth: 1.5))
        }
    }

    /// A cycle covers days start…end, so its step spans x = start…end + 1.
    @ChartContentBuilder private var lowsMarks: some ChartContent {
        ForEach(spans, id: \.cycle.start) { span in
            let q = span.cycle.quantiles
            let fade = span.cycle.partial ? 0.45 : 1
            RectangleMark(
                xStart: .value("Day", span.start),
                xEnd: .value("Day", span.end + 1),
                yStart: .value("Balance", q[band.lo]),
                yEnd: .value("Balance", q[band.hi])
            )
            .foregroundStyle(Theme.bandOuter)
            .opacity(fade)
            RectangleMark(
                xStart: .value("Day", span.start),
                xEnd: .value("Day", span.end + 1),
                yStart: .value("Balance", q[band.qlo]),
                yEnd: .value("Balance", q[band.qhi])
            )
            .foregroundStyle(Theme.bandInner)
            .opacity(fade)
            RuleMark(
                xStart: .value("Day", span.start),
                xEnd: .value("Day", span.end + 1),
                y: .value("Balance", q[band.mid])
            )
            .foregroundStyle(Theme.median)
            .opacity(fade)
            .lineStyle(StrokeStyle(lineWidth: 2))
        }
    }

    private var xTicks: [Int] {
        let n = days.count
        let step = max(1, Int((Double(n) / 4).rounded()))
        return Array(stride(from: 0, through: n, by: step))
    }

    private func tooltip(day i: Int) -> some View {
        let levels = report.forecast.levels
        let cycle = view == .lows ? spans.first { $0.start <= i && i <= $0.end }?.cycle : nil
        let title = cycle.map {
            "Lowest, \(Format.shortDate($0.start)) – \(Format.shortDate($0.end))\($0.partial ? " (partial cycle)" : "")"
        } ?? Format.longDate(days[i].date)
        let q = cycle?.quantiles ?? days[i].quantiles
        return VStack(alignment: .leading, spacing: 3) {
            Text(title).font(.caption.weight(.semibold))
            ForEach(Array(levels.enumerated()), id: \.offset) { k, l in
                HStack {
                    Text("\(Int((l * 100).rounded()))%").foregroundStyle(Theme.dim)
                    Spacer(minLength: 16)
                    Text(Format.money(q[k])).monospacedDigit()
                }
                .font(.caption.monospaced())
            }
        }
        .padding(10)
        .frame(width: 190)
        .background(Theme.raised, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.line))
    }
}
