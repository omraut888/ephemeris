// The headline: the chance the balance goes below $0 on any day of the
// horizon, read from the engine's `below_by` (web/src/components/Risk.tsx).

import EphemerisKit
import SwiftUI

struct RiskView: View {
    let report: ForecastReport

    var body: some View {
        if let p = Risk.belowZero(report), let last = report.forecast.days.last {
            let color = switch Risk.tone(p) {
            case .good: Theme.good
            case .warn: Theme.warn
            case .bad: Theme.bad
            }
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(Format.percent(p))
                    .font(.system(.largeTitle, design: .monospaced).weight(.semibold))
                    .foregroundStyle(color)
                (Text("chance the balance dips below $0 at any point through \(Format.longDate(last.date))")
                    + Text(" · end-of-day balances, \(report.forecast.paths.formatted()) simulated paths")
                    .foregroundStyle(Theme.dim))
                    .font(.subheadline)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(14)
            .background(color.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
            .overlay(alignment: .leading) {
                RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 3).padding(.vertical, 6)
            }
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("risk")
        }
    }
}
