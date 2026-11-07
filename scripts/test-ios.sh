#!/usr/bin/env bash
# The iOS app in the simulator (docs/phase3-design.md §6):
#
# 1. EphemerisKit's tests on the iOS simulator: the golden feed and report
#    through the FFI on iOS itself (the fourth byte-identity path), the drift,
#    format, lane, app-model and sync-device tests.
# 2. The app installed and launched with `-EPHEMERIS_SYNC_SELFTEST YES`: it
#    first runs a sync round trip through the iOS Keychain (a throwaway
#    service, deleted after) and a coordinated folder, and must log
#    "EPHEMERIS sync selftest ok"; then "EPHEMERIS ready" (a demo forecast
#    on screen), and no "EPHEMERIS error".
#
# Uses the dedicated simulator from scripts/ios-device.sh and shuts it down
# afterwards unless KEEP_BOOTED=1. The app gets LAUNCH_TIMEOUT seconds
# (default 300) to log. Needs Xcode and an installed iOS simulator runtime
# (`xcodebuild -downloadPlatform iOS`).
set -euo pipefail
cd "$(dirname "$0")/.."

source scripts/ios-device.sh

scripts/build-ios.sh --sim-only
xcodegen generate --spec ios/project.yml --quiet

trap shutdown_device EXIT

echo "== EphemerisKit tests on $DEVICE_NAME"
xcodebuild test -project ios/Ephemeris.xcodeproj -scheme Ephemeris \
  -destination "id=$udid" -derivedDataPath "$DERIVED" -quiet

echo "== launching the app"
boot_device
xcrun simctl install "$udid" "$APP"
log=$(mktemp)
trap 'stop_app; rm -f "$log"; shutdown_device' EXIT
launch_and_wait "$log" -EPHEMERIS_SYNC_SELFTEST YES
stop_app
grep "EPHEMERIS sync selftest ok" "$log"
grep "EPHEMERIS ready" "$log"
