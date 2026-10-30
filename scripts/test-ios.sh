#!/usr/bin/env bash
# The iOS app in the simulator (docs/phase3-design.md §6):
#
# 1. EphemerisKit's tests on the iOS simulator: the golden feed and report
#    through the FFI on iOS itself (the fourth byte-identity path), the drift,
#    format, lane and app-model tests.
# 2. The app installed and launched; it must log "EPHEMERIS ready" (a demo
#    forecast on screen) and no "EPHEMERIS error".
#
# Uses one dedicated simulator, "EPHEMERIS iPhone", created on first run as
# the first iPhone model the newest iOS runtime supports (DEVICE_TYPE
# overrides it), and shuts it down afterwards unless
# KEEP_BOOTED=1. Needs Xcode and an installed iOS simulator runtime
# (`xcodebuild -downloadPlatform iOS`).
set -euo pipefail
cd "$(dirname "$0")/.."

DEVICE_NAME="EPHEMERIS iPhone"
DERIVED=ios/build/DerivedData
APP=$DERIVED/Build/Products/Debug-iphonesimulator/Ephemeris.app
BUNDLE_ID=dev.ephemeris.ios

read -r runtime default_type < <(xcrun simctl list runtimes available -j | python3 -c '
import json, sys
rts = [r for r in json.load(sys.stdin)["runtimes"] if r["platform"] == "iOS"]
if rts:
    phones = [t["identifier"] for t in rts[-1]["supportedDeviceTypes"] if t["productFamily"] == "iPhone"]
    print(rts[-1]["identifier"], phones[0] if phones else "")') || true
DEVICE_TYPE=${DEVICE_TYPE:-${default_type:-}}
if [ -z "${runtime:-}" ] || [ -z "$DEVICE_TYPE" ]; then
  echo "no iOS simulator runtime: run xcodebuild -downloadPlatform iOS" >&2
  exit 1
fi
udid=$(xcrun simctl list devices available -j | python3 -c '
import json, sys
name, runtime = sys.argv[1], sys.argv[2]
for d in json.load(sys.stdin)["devices"].get(runtime, []):
    if d["name"] == name:
        print(d["udid"]); break' "$DEVICE_NAME" "$runtime")
if [ -z "$udid" ]; then
  udid=$(xcrun simctl create "$DEVICE_NAME" "$DEVICE_TYPE" "$runtime")
  echo "== created $DEVICE_NAME ($udid)"
fi

scripts/build-ios.sh --sim-only
xcodegen generate --spec ios/project.yml --quiet

cleanup() {
  [ "${KEEP_BOOTED:-0}" = 1 ] || xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "== EphemerisKit tests on $DEVICE_NAME"
xcodebuild test -project ios/Ephemeris.xcodeproj -scheme Ephemeris \
  -destination "id=$udid" -derivedDataPath "$DERIVED" -quiet

echo "== launching the app"
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b >/dev/null
xcrun simctl install "$udid" "$APP"
log=$(mktemp)
xcrun simctl launch --console --terminate-running-process "$udid" "$BUNDLE_ID" >"$log" 2>&1 &
launcher=$!
for _ in $(seq 1 120); do
  grep -q "EPHEMERIS \(ready\|error\)" "$log" && break
  sleep 0.5
done
kill "$launcher" 2>/dev/null || true
xcrun simctl terminate "$udid" "$BUNDLE_ID" 2>/dev/null || true
if grep -q "EPHEMERIS error" "$log" || ! grep -q "EPHEMERIS ready" "$log"; then
  echo "the app did not reach a forecast:" >&2
  cat "$log" >&2
  rm -f "$log"
  exit 1
fi
grep "EPHEMERIS ready" "$log"
rm -f "$log"
