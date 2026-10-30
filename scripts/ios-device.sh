# Sourced by test-ios.sh and ios-screenshots.sh: finds the dedicated
# simulator, "EPHEMERIS iPhone", and sets `udid`. It is created on first use
# as the first iPhone model the newest iOS runtime supports (DEVICE_TYPE
# overrides it). Needs an installed iOS simulator runtime
# (`xcodebuild -downloadPlatform iOS`).

DEVICE_NAME="EPHEMERIS iPhone"
BUNDLE_ID=dev.ephemeris.ios
DERIVED=ios/build/DerivedData
APP=$DERIVED/Build/Products/Debug-iphonesimulator/Ephemeris.app
# On an 8 GB Mac the simulator keeps the machine saturated for minutes after
# a boot: a launch measured 214 s cold and 85 s warm (docs/phase3-design.md,
# As built).
LAUNCH_TIMEOUT=${LAUNCH_TIMEOUT:-300}

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

shutdown_device() {
  [ "${KEEP_BOOTED:-0}" = 1 ] || xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true
}

boot_device() {
  xcrun simctl boot "$udid" 2>/dev/null || true
  xcrun simctl bootstatus "$udid" -b >/dev/null
}

# launch_and_wait LOG [ARGS...]: launches the app with ARGS and waits up to
# LAUNCH_TIMEOUT for its "EPHEMERIS ready" or "EPHEMERIS error" line. Fails
# unless it logged "ready" and no error. The app runs only as long as the
# console launcher does, so that is left running: `stop_app` ends both.
launcher=
launch_and_wait() {
  local log=$1
  shift
  xcrun simctl launch --console --terminate-running-process "$udid" "$BUNDLE_ID" "$@" >"$log" 2>&1 &
  launcher=$!
  for _ in $(seq 1 $((LAUNCH_TIMEOUT * 2))); do
    grep -q "EPHEMERIS \(ready\|error\)" "$log" && break
    sleep 0.5
  done
  if grep -q "EPHEMERIS error" "$log" || ! grep -q "EPHEMERIS ready" "$log"; then
    stop_app
    echo "the app did not reach a forecast within ${LAUNCH_TIMEOUT}s:" >&2
    cat "$log" >&2
    return 1
  fi
}

stop_app() {
  [ -n "$launcher" ] && kill "$launcher" 2>/dev/null || true
  launcher=
  xcrun simctl terminate "$udid" "$BUNDLE_ID" >/dev/null 2>&1 || true
}

# app_screenshot FILE: the screen the app is on. Some device types have
# more than one display (iPhone Duo: an inner panel, black while folded, and
# the cover screen the app draws on), and `screenshot` alone takes the
# first. So every display with a framebuffer is captured and the largest
# PNG kept: a black frame compresses to about 100 KB.
app_screenshot() {
  local file=$1 tmp best=0 size
  tmp=$(mktemp -d)
  for display in $(xcrun simctl io "$udid" enumerate | awk '
      /UUID:/ { uuid = $2 }
      /IOSurface port:/ { print uuid }'); do
    xcrun simctl io "$udid" screenshot --display="$display" "$tmp/$display.png" >/dev/null 2>&1 || continue
    size=$(stat -f %z "$tmp/$display.png")
    if [ "$size" -gt "$best" ]; then
      best=$size
      mv "$tmp/$display.png" "$file"
    fi
  done
  rm -rf "$tmp"
  [ "$best" -gt 0 ]
}
