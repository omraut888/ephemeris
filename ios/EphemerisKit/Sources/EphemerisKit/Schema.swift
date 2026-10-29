// The report and options JSON (docs/forecast-schema.md) as Codable types,
// written by hand to mirror crates/ephemeris-core/src/schema.rs and
// pipeline.rs (docs/phase3-design.md §1). DriftTests round-trips the golden
// report and the all-fields fixture through them, so a schema change that is
// not mirrored here fails `swift test`.
//
// Dates stay `IsoDate` strings (`YYYY-MM-DD`), as in TypeScript: the app
// formats them but never does arithmetic on them. Feeds are not decoded at
// all; they pass through as text.

/// `YYYY-MM-DD`.
public typealias IsoDate = String

public struct ForecastReport: Codable, Equatable, Sendable {
    public var schemaVersion: Int
    /// The last day of data; the forecast starts the day after.
    public var asOf: IsoDate
    /// Days after this still hold pending amounts.
    public var settledThrough: IsoDate
    public var balance: Estimate
    public var spendRate: Estimate
    /// Present when the forecast floors the spend rate; replaces its ±2σ.
    public var spendRateRange: SpendRange?
    public var expectedPaycheck: Estimate
    public var income: IncomeReport?
    public var bills: [BillReport]
    public var regimeChanges: [RegimeChange]
    public var forecast: ConeReport

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case asOf = "as_of"
        case settledThrough = "settled_through"
        case balance
        case spendRate = "spend_rate"
        case spendRateRange = "spend_rate_range"
        case expectedPaycheck = "expected_paycheck"
        case income, bills
        case regimeChanges = "regime_changes"
        case forecast
    }
}

public struct Estimate: Codable, Equatable, Sendable {
    public var mean: Double
    public var sd: Double
    public var low2sd: Double
    public var high2sd: Double

    enum CodingKeys: String, CodingKey {
        case mean, sd
        case low2sd = "low_2sd"
        case high2sd = "high_2sd"
    }
}

public struct SpendRange: Codable, Equatable, Sendable {
    public var floor: Double
    public var low: Double
    public var high: Double
    /// Share of the untruncated Gaussian below the floor.
    public var belowFloor: Double

    enum CodingKeys: String, CodingKey {
        case floor, low, high
        case belowFloor = "below_floor"
    }
}

public struct IncomeReport: Codable, Equatable, Sendable {
    public var name: String?
    public var cadence: PayCadence
    public var lastPaidNominal: IsoDate
    public var nextScheduled: IsoDate?
    public var offsetProbabilities: [Double]
    public var fitCost: Int

    enum CodingKeys: String, CodingKey {
        case name, cadence
        case lastPaidNominal = "last_paid_nominal"
        case nextScheduled = "next_scheduled"
        case offsetProbabilities = "offset_probabilities"
        case fitCost = "fit_cost"
    }
}

/// Tagged by `kind`, as serde writes it.
public enum PayCadence: Codable, Equatable, Sendable {
    case weekly(anchor: IsoDate)
    case biweekly(anchor: IsoDate)
    case semiMonthly(days: [Int])
    case monthly(day: Int)

    enum CodingKeys: String, CodingKey {
        case kind, anchor, days, day
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "weekly": self = .weekly(anchor: try c.decode(IsoDate.self, forKey: .anchor))
        case "biweekly": self = .biweekly(anchor: try c.decode(IsoDate.self, forKey: .anchor))
        case "semi_monthly": self = .semiMonthly(days: try c.decode([Int].self, forKey: .days))
        case "monthly": self = .monthly(day: try c.decode(Int.self, forKey: .day))
        case let other:
            throw DecodingError.dataCorruptedError(
                forKey: .kind, in: c, debugDescription: "unknown pay cadence \(other)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .weekly(anchor):
            try c.encode("weekly", forKey: .kind)
            try c.encode(anchor, forKey: .anchor)
        case let .biweekly(anchor):
            try c.encode("biweekly", forKey: .kind)
            try c.encode(anchor, forKey: .anchor)
        case let .semiMonthly(days):
            try c.encode("semi_monthly", forKey: .kind)
            try c.encode(days, forKey: .days)
        case let .monthly(day):
            try c.encode("monthly", forKey: .kind)
            try c.encode(day, forKey: .day)
        }
    }
}

public struct BillReport: Codable, Equatable, Sendable {
    public var stream: Int
    public var name: String?
    /// Signed: inflows positive.
    public var amount: Double
    public var periodDays: Int
    public var monthlyOn: MonthlyOn?
    public var lastCharged: IsoDate
    public var nextDue: IsoDate

    enum CodingKeys: String, CodingKey {
        case stream, name, amount
        case periodDays = "period_days"
        case monthlyOn = "monthly_on"
        case lastCharged = "last_charged"
        case nextDue = "next_due"
    }
}

public struct MonthlyOn: Codable, Equatable, Sendable {
    public var day: Int
    public var shift: Shift
}

public enum Shift: String, Codable, Sendable {
    case none
    case previousBusinessDay = "previous_business_day"
    case nextBusinessDay = "next_business_day"
}

public struct RegimeChange: Codable, Equatable, Sendable {
    public var date: IsoDate
    public var settled: Bool
    public var channel: ChangeChannel
    public var shift: Double
}

public enum ChangeChannel: String, Codable, Sendable {
    case spend, income
}

public struct ConeReport: Codable, Equatable, Sendable {
    public var horizonDays: Int
    public var paths: Int
    public var seed: UInt64
    public var levels: [Double]
    public var days: [ConeDay]
    public var targets: [TargetReport]
    public var cycleLows: CycleLowsReport?

    enum CodingKeys: String, CodingKey {
        case horizonDays = "horizon_days"
        case paths, seed, levels, days, targets
        case cycleLows = "cycle_lows"
    }
}

public struct ConeDay: Codable, Equatable, Sendable {
    public var date: IsoDate
    /// One per `ConeReport.levels`.
    public var quantiles: [Double]
}

public struct TargetReport: Codable, Equatable, Sendable {
    public var amount: Double
    /// P(balance ≥ amount on day i).
    public var atLeastOnDay: [Double]
    /// P(balance ≥ amount on some day ≤ i).
    public var reachedBy: [Double]
    /// P(balance < amount on some day ≤ i).
    public var belowBy: [Double]

    enum CodingKeys: String, CodingKey {
        case amount
        case atLeastOnDay = "at_least_on_day"
        case reachedBy = "reached_by"
        case belowBy = "below_by"
    }
}

public struct CycleLowsReport: Codable, Equatable, Sendable {
    public var basis: CycleBasisReport
    public var cycles: [CycleLowReport]
}

public struct CycleLowReport: Codable, Equatable, Sendable {
    public var start: IsoDate
    public var end: IsoDate
    /// Cut by today or the horizon end.
    public var partial: Bool
    public var quantiles: [Double]
}

public enum CycleBasisReport: String, Codable, Sendable {
    case paycheck, week
}

// MARK: - Options (encoded only; the engine rejects unknown fields)

public enum SimMode: String, Codable, Sendable, CaseIterable {
    case realistic, matched
}

/// Every field optional, like TypeScript's `Partial<SimulateOptions>`: the
/// engine fills in the rest.
public struct SimulateOptions: Codable, Equatable, Sendable {
    public var seed: UInt64?
    public var days: Int?
    public var mode: SimMode?
    public var start: IsoDate?

    public init(seed: UInt64? = nil, days: Int? = nil, mode: SimMode? = nil, start: IsoDate? = nil) {
        self.seed = seed
        self.days = days
        self.mode = mode
        self.start = start
    }
}

public struct ForecastOptions: Codable, Equatable, Sendable {
    public var horizonDays: Int?
    public var paths: Int?
    public var seed: UInt64?
    public var targets: [Double]?

    public init(horizonDays: Int? = nil, paths: Int? = nil, seed: UInt64? = nil, targets: [Double]? = nil) {
        self.horizonDays = horizonDays
        self.paths = paths
        self.seed = seed
        self.targets = targets
    }

    enum CodingKeys: String, CodingKey {
        case horizonDays = "horizon_days"
        case paths, seed, targets
    }
}
