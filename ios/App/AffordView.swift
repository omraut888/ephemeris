// "Can I afford $X by date D?" (web/src/components/AffordQuery.tsx). The
// model reruns the same forecast with X as the target; this view reads the
// two probabilities for day D and words them.

import EphemerisKit
import SwiftUI

struct AffordView: View {
    let model: AppModel
    let report: ForecastReport
    @State private var amount = "2000"
    @State private var date: Date
    @State private var answer: AffordAnswer?
    @State private var error: String?
    @State private var busy = false

    init(model: AppModel, report: ForecastReport) {
        self.model = model
        self.report = report
        let days = report.forecast.days
        let initial = days.isEmpty ? report.asOf : days[min(29, days.count - 1)].date
        _date = State(initialValue: IsoDay.date(initial) ?? .now)
    }

    private var range: ClosedRange<Date> {
        let days = report.forecast.days
        let first = IsoDay.date(days.first?.date ?? report.asOf) ?? .now
        let last = IsoDay.date(days.last?.date ?? report.asOf) ?? first
        return first...max(first, last)
    }

    var body: some View {
        Panel {
            SectionLabel("Can I afford it?")
            HStack(spacing: 8) {
                Text("Have at least $").foregroundStyle(Theme.dim)
                TextField("Amount", text: $amount)
                    .keyboardType(.decimalPad)
                    .textFieldStyle(.roundedBorder)
                    .font(.body.monospacedDigit())
                    .frame(maxWidth: 110)
                    .accessibilityIdentifier("afford-amount")
            }
            HStack {
                DatePicker("on", selection: $date, in: range, displayedComponents: .date)
                    .foregroundStyle(Theme.dim)
                    .environment(\.timeZone, IsoDay.timeZone)
                Spacer()
                Button(busy ? "Checking…" : "Check") { Task { await check() } }
                    .buttonStyle(.borderedProminent)
                    .disabled(busy)
                    .accessibilityIdentifier("afford-check")
            }
            if let error {
                Text(error).font(.callout).foregroundStyle(Theme.bad)
            } else if let answer {
                card(
                    "On \(Format.longDate(answer.date))",
                    answer.onDay,
                    "chance of at least \(Format.money(answer.amount)) at the end of that day"
                )
                card(
                    "At any point through \(Format.longDate(answer.date))",
                    answer.reachedBy,
                    "chance of at least \(Format.money(answer.amount)) at the end of some day from today through then"
                )
                Text(
                    "Why they differ: the balance swings, so a paycheck can lift it past \(Format.money(answer.amount)) for a few days before bills pull it back under; the second number is never the lower one."
                )
                .font(.caption)
                .foregroundStyle(Theme.dim)
            }
        }
    }

    private func card(_ title: String, _ p: Double, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            SectionLabel(title)
            Text(Format.percent(p)).font(.system(.title, design: .monospaced).weight(.semibold))
            Text(detail).font(.footnote).foregroundStyle(Theme.dim)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(Theme.raised, in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .combine)
    }

    private func check() async {
        guard let x = Double(amount.replacingOccurrences(of: ",", with: "")), x.isFinite else {
            error = "Enter an amount in dollars."
            return
        }
        let day = IsoDay.string(date)
        busy = true
        defer { busy = false }
        do {
            if let a = try await model.afford(amount: x, date: day) {
                answer = a
                error = nil
            } else {
                let days = report.forecast.days
                error = "Pick a date from \(Format.longDate(days.first?.date ?? report.asOf)) to \(Format.longDate(days.last?.date ?? report.asOf))."
            }
        } catch is SupersededError {
            // A newer check replaced this one.
        } catch {
            self.error = errorMessage(error)
        }
    }
}

/// `YYYY-MM-DD` to and from the picker's `Date`, in UTC so a day is never
/// shifted by the device's time zone. Conversion for the picker only.
enum IsoDay {
    static let timeZone = TimeZone(identifier: "UTC")!

    private static let formatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = timeZone
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    static func date(_ s: IsoDate) -> Date? { formatter.date(from: s) }
    static func string(_ d: Date) -> IsoDate { formatter.string(from: d) }
}
