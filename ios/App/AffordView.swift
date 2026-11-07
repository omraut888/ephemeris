// "Can I afford $X by date D?" (web/src/components/AffordQuery.tsx). The
// model reruns the same forecast with X as the target; this view reads the
// two probabilities for day D and words them.

import EphemerisKit
import SwiftUI

struct AffordView: View {
    let model: AppModel
    let report: ForecastReport
    /// Saved queries sync between devices once this iPhone is paired.
    let sync: SyncModel?
    @State private var amount = "2000"
    @State private var date: Date
    @State private var label = ""
    @State private var answer: AffordAnswer?
    @State private var error: String?
    @State private var busy = false

    init(model: AppModel, report: ForecastReport, sync: SyncModel? = nil) {
        self.model = model
        self.report = report
        self.sync = sync
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
            if let sync, sync.status.paired {
                HStack(spacing: 8) {
                    TextField("label (optional)", text: $label)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("afford-label")
                    Button("Save") { Task { await save(sync) } }
                        .buttonStyle(.bordered)
                        .disabled(sync.busy)
                        .accessibilityIdentifier("afford-save")
                }
                ForEach(sync.queries) { q in
                    HStack {
                        Button {
                            amount = Self.dollars(q.amountCents)
                            if let d = IsoDay.date(q.by) { date = d }
                        } label: {
                            Text("\(Format.money(Double(q.amountCents) / 100)) by \(Format.longDate(q.by))\(q.label.isEmpty ? "" : " · \(q.label)")")
                                .font(.callout)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(.borderless)
                        Button {
                            Task { await sync.deleteQuery(id: q.id) }
                        } label: {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.dim)
                        }
                        .buttonStyle(.borderless)
                        .accessibilityLabel("Delete saved query \(q.label.isEmpty ? q.id : q.label)")
                    }
                }
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

    /// Save the question as it stands, for every paired device.
    private func save(_ sync: SyncModel) async {
        guard let x = Double(amount.replacingOccurrences(of: ",", with: "")), x.isFinite else {
            error = "Enter an amount in dollars."
            return
        }
        error = nil
        await sync.saveQuery(amountCents: Int64((x * 100).rounded()), by: IsoDay.string(date), label: label.trimmingCharacters(in: .whitespaces))
        if sync.status.error == nil { label = "" } else { error = sync.status.error }
    }

    /// Cents as the amount field shows dollars: "2000", "12.5".
    static func dollars(_ cents: Int64) -> String {
        cents % 100 == 0 ? String(cents / 100) : String(Double(cents) / 100)
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
