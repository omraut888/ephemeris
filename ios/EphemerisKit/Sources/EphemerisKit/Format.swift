// Formatting only, ported from web/src/format.ts so both apps print the same
// strings (docs/phase3-design.md §3): turning numbers and ISO date strings
// the engine produced into text. No arithmetic on forecast data happens here.

import Foundation

public enum Format {
    private static let enUS = Locale(identifier: "en_US")

    // One rule for money. Every amount in text (balances, ±2σ, paychecks,
    // bills, the afford query) shows cents: `$21,967.00` is printed only when
    // the value really is within half a cent of it. Chart axes show whole
    // dollars, since their ticks are round numbers chosen for the scale.
    // ICU rounding, half away from zero, as the web's Intl does.
    private static let cents = FloatingPointFormatStyle<Double>.Currency(code: "USD", locale: enUS)
        .precision(.fractionLength(2))
        .rounded(rule: .toNearestOrAwayFromZero)
    private static let wholeDollars = FloatingPointFormatStyle<Double>.Currency(code: "USD", locale: enUS)
        .precision(.fractionLength(0))
        .rounded(rule: .toNearestOrAwayFromZero)

    /// A zero amount has no sign (the formatter renders −0 as "-$0").
    private static func unsignedZero(_ s: String) -> String {
        s == "-$0" || s == "-$0.00" ? String(s.dropFirst()) : s
    }

    /// `$1,234.56`; `-$48.20` for negatives.
    public static func money(_ x: Double) -> String {
        unsignedZero(x.formatted(cents))
    }

    /// `+$31.20` or `-$97.60`: a change, always signed.
    public static func signedMoney(_ x: Double) -> String {
        let s = money(abs(x))
        return x < 0 ? "-\(s)" : "+\(s)"
    }

    /// `$28,000`: whole dollars, for chart axis ticks only.
    public static func moneyAxis(_ x: Double) -> String {
        unsignedZero(x.formatted(wholeDollars))
    }

    /// `61%`; extremes as `<1%` and `>99%` so 0 and 1 never look certain.
    public static func percent(_ p: Double) -> String {
        if p <= 0 { return "<1%" }
        if p >= 1 { return ">99%" }
        let s = Int((p * 100).rounded())
        if s < 1 { return "<1%" }
        if s > 99 { return ">99%" }
        return "\(s)%"
    }

    private static let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]

    private static func part(_ d: IsoDate, _ from: Int, _ count: Int) -> Int {
        let start = d.index(d.startIndex, offsetBy: from, limitedBy: d.endIndex) ?? d.endIndex
        let end = d.index(start, offsetBy: count, limitedBy: d.endIndex) ?? d.endIndex
        return Int(d[start..<end]) ?? 0
    }

    /// `Oct 22` from `2026-10-22`.
    public static func shortDate(_ d: IsoDate) -> String {
        let m = part(d, 5, 2)
        let month = (1...12).contains(m) ? months[m - 1] : "?"
        return "\(month) \(part(d, 8, 2))"
    }

    /// `Oct 22, 2026`.
    public static func longDate(_ d: IsoDate) -> String {
        "\(shortDate(d)), \(d.prefix(4))"
    }

    /// `1st`, `22nd`, `31st`.
    public static func ordinal(_ n: Int) -> String {
        let tens = n % 100
        if (11...13).contains(tens) { return "\(n)th" }
        switch n % 10 {
        case 1: return "\(n)st"
        case 2: return "\(n)nd"
        case 3: return "\(n)rd"
        default: return "\(n)th"
        }
    }

    public static func cadence(_ c: PayCadence) -> String {
        switch c {
        case .weekly: return "weekly"
        case .biweekly: return "every 2 weeks"
        case let .semiMonthly(days):
            return "on the \(ordinal(days.first ?? 0)) and \(ordinal(days.last ?? 0))"
        case let .monthly(day): return "monthly on the \(ordinal(day))"
        }
    }
}
