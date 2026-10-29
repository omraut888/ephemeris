// The web app's palette (web/src/styles.css), dark first, following the
// system appearance.

import SwiftUI
import UIKit

extension Color {
    /// A color that follows light and dark mode.
    init(light: UInt32, dark: UInt32, opacity: (light: Double, dark: Double) = (1, 1)) {
        func ui(_ hex: UInt32, _ alpha: Double) -> UIColor {
            UIColor(
                red: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: alpha
            )
        }
        self.init(uiColor: UIColor { traits in
            traits.userInterfaceStyle == .dark ? ui(dark, opacity.dark) : ui(light, opacity.light)
        })
    }
}

enum Theme {
    static let bg = Color(light: 0xF6F7F9, dark: 0x0A0C10)
    static let panel = Color(light: 0xFFFFFF, dark: 0x0F1218)
    static let raised = Color(light: 0xF1F3F6, dark: 0x141821)
    static let line = Color(light: 0xE3E6EC, dark: 0x1F2430)
    static let text = Color(light: 0x12151B, dark: 0xE6E9EF)
    static let dim = Color(light: 0x545D70, dark: 0x8A93A6)
    static let faint = Color(light: 0x8A93A6, dark: 0x5B6478)
    static let accent = Color(light: 0x2F6FE0, dark: 0x6EA8FE)
    static let bandOuter = Color(light: 0x2F6FE0, dark: 0x6EA8FE, opacity: (0.12, 0.16))
    static let bandInner = Color(light: 0x2F6FE0, dark: 0x6EA8FE, opacity: (0.28, 0.34))
    static let median = Color(light: 0x1C3F86, dark: 0xDBE7FF)
    static let warn = Color(light: 0xB7791F, dark: 0xF5B454)
    static let bad = Color(light: 0xC53030, dark: 0xFF7A7A)
    static let good = Color(light: 0x1F8A5B, dark: 0x63D6A4)
}

/// The small uppercase section labels.
struct SectionLabel: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text.uppercased())
            .font(.caption.weight(.semibold))
            .tracking(1.2)
            .foregroundStyle(Theme.dim)
    }
}

/// A rounded panel, as the web's sections.
struct Panel<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) { content }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(14)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.line))
    }
}
