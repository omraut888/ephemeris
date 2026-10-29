// Reading values out of a report for display, as the web components do:
// picking indices and looking entries up. Nothing here computes a
// probability or a quantile; the engine already did.

/// Which entries of `levels` are the outer band, inner band and median
/// (web/src/components/ConeChart.tsx, `bandIndex`).
public struct BandIndex: Equatable, Sendable {
    public var lo: Int
    public var qlo: Int
    public var mid: Int
    public var qhi: Int
    public var hi: Int

    public init(levels: [Double]) {
        func nearest(_ target: Double) -> Int {
            levels.indices.reduce(0) { best, i in
                abs(levels[i] - target) < abs(levels[best] - target) ? i : best
            }
        }
        lo = 0
        qlo = nearest(0.25)
        mid = nearest(0.5)
        qhi = nearest(0.75)
        hi = max(levels.count - 1, 0)
    }
}

/// The headline (web/src/components/Risk.tsx).
public enum Risk {
    /// The target every forecast requests, so the headline has a curve.
    public static let overdrawnTarget = 0.0

    public enum Tone: Sendable {
        case good, warn, bad
    }

    /// P(balance below $0 on some day of the horizon): the engine's
    /// `below_by` for target 0, on the last day.
    public static func belowZero(_ report: ForecastReport) -> Double? {
        report.forecast.targets.first { $0.amount == overdrawnTarget }?.belowBy.last
    }

    /// Display tiers only.
    public static func tone(_ p: Double) -> Tone {
        p >= 0.2 ? .bad : p >= 0.05 ? .warn : .good
    }
}

/// The two afford probabilities for one amount and day
/// (web/src/components/AffordQuery.tsx).
public struct AffordAnswer: Equatable, Sendable {
    public var amount: Double
    public var date: IsoDate
    /// P(balance ≥ amount at the end of that day).
    public var onDay: Double
    /// P(balance ≥ amount at the end of some day from today through then).
    public var reachedBy: Double

    /// From a report run with `targets: [amount]`; nil when the date is not
    /// one of its days.
    public init?(report: ForecastReport, amount: Double, date: IsoDate) {
        guard let i = report.forecast.days.firstIndex(where: { $0.date == date }),
              let t = report.forecast.targets.first,
              i < t.atLeastOnDay.count, i < t.reachedBy.count
        else { return nil }
        self.amount = amount
        self.date = date
        onDay = t.atLeastOnDay[i]
        reachedBy = t.reachedBy[i]
    }
}
