// The web's format cases (web/test/format.test.ts), so both apps print the
// same strings.

import Testing

@testable import EphemerisKit

@Suite struct FormatTests {
    @Test func money() {
        #expect(Format.money(1234.56) == "$1,234.56")
        #expect(Format.money(-48.2) == "-$48.20")
        #expect(Format.money(-0.001) == "$0.00")
        // The demo balance: really $21,966.9998, so ".00" is not a rounding artifact.
        #expect(Format.money(21966.99978818912) == "$21,967.00")
        #expect(Format.money(21966.99) == "$21,966.99")
        #expect(Format.signedMoney(31.2) == "+$31.20")
        #expect(Format.signedMoney(-97.6) == "-$97.60")
        #expect(Format.moneyAxis(28000) == "$28,000")
        #expect(Format.moneyAxis(-500) == "-$500")
        #expect(Format.moneyAxis(-0.0) == "$0")
    }

    @Test func neverPrintsAProbabilityAsCertain() {
        #expect(Format.percent(0) == "<1%")
        #expect(Format.percent(0.004) == "<1%")
        #expect(Format.percent(0.61) == "61%")
        #expect(Format.percent(0.996) == ">99%")
        #expect(Format.percent(1) == ">99%")
    }

    @Test func datesAndCadencesFromTheReportsStrings() {
        #expect(Format.shortDate("2026-10-02") == "Oct 2")
        #expect(Format.longDate("2024-12-31") == "Dec 31, 2024")
        #expect([1, 2, 3, 4, 11, 12, 13, 21, 22, 31].map(Format.ordinal).joined(separator: " ")
            == "1st 2nd 3rd 4th 11th 12th 13th 21st 22nd 31st")
        #expect(Format.cadence(.semiMonthly(days: [15, 31])) == "on the 15th and 31st")
        #expect(Format.cadence(.monthly(day: 1)) == "monthly on the 1st")
        #expect(Format.cadence(.biweekly(anchor: "2024-01-05")) == "every 2 weeks")
    }
}
