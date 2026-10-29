// The chart's header (view and horizon), the cone, and its legend.

import EphemerisKit
import SwiftUI

struct ForecastPanel: View {
    @Bindable var model: AppModel
    let report: ForecastReport

    var body: some View {
        Panel {
            SectionLabel("Forecast · \(report.forecast.horizonDays) days · \(report.forecast.paths.formatted()) paths")
            Picker("Cone view", selection: $model.view) {
                Text("Daily").tag(AppModel.ConeView.daily)
                Text("Pay-cycle lows").tag(AppModel.ConeView.lows)
            }
            .pickerStyle(.segmented)
            .disabled(report.forecast.cycleLows == nil)
            Picker("Horizon", selection: Binding(get: { model.horizon }, set: { model.setHorizon($0) })) {
                ForEach(AppModel.horizons, id: \.self) { Text("\($0) days").tag($0) }
            }
            .pickerStyle(.segmented)

            ConeChartView(report: report, view: model.view)

            legend
        }
    }

    private var legend: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 14) {
                swatch(Theme.bandOuter, "5–95%")
                swatch(Theme.bandInner, "25–75%")
                HStack(spacing: 5) {
                    Capsule().fill(Theme.median).frame(width: 14, height: 2)
                    Text("median")
                }
                if model.view == .lows, report.forecast.cycleLows?.cycles.contains(where: \.partial) == true {
                    HStack(spacing: 5) {
                        RoundedRectangle(cornerRadius: 2).fill(Theme.bandInner.opacity(0.45)).frame(width: 12, height: 10)
                        Text("partial")
                    }
                }
            }
            Text(caption)
        }
        .font(.caption)
        .foregroundStyle(Theme.dim)
    }

    private var caption: String {
        switch model.view {
        case .daily: return "End-of-day balance, each day."
        case .lows:
            let unit = report.forecast.cycleLows?.basis == .week ? "week" : "pay cycle"
            let partial = report.forecast.cycleLows?.cycles.contains(where: \.partial) == true
                ? " Faded: a partial cycle, cut by today or the horizon end." : ""
            return "Each path's lowest balance in each \(unit).\(partial)"
        }
    }

    private func swatch(_ color: Color, _ label: String) -> some View {
        HStack(spacing: 5) {
            RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 12, height: 10)
            Text(label)
        }
    }
}
