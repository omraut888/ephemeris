#!/usr/bin/env bash
# Screenshots of the iOS app in the simulator, for the README and for
# checking both appearances (docs/phase3-design.md §6).
#
#   scripts/ios-screenshots.sh                 # the README set → docs/img/ios-*.png
#   scripts/ios-screenshots.sh --all --out DIR # every combination, to review
#   scripts/ios-screenshots.sh --no-build      # reuse the last app build
#
# It builds the app, boots the simulator from scripts/ios-device.sh, sets a
# clean status bar, and for each shot sets the appearance, launches the app
# with the Debug-only arguments (-EPHEMERIS_FEED, -EPHEMERIS_VIEW), waits for
# its "EPHEMERIS ready" line and for the screen to settle, and takes the
# screenshot of the display the app is on. The sandbox shots load
# data/sandbox/feed.json (written by scripts/fetch_plaid_sandbox.py, not
# committed), copied into the app's Documents; without it they are skipped.
# Fails if the app logs an error or does not get ready within
# LAUNCH_TIMEOUT seconds. Shuts the simulator down unless KEEP_BOOTED=1.
set -euo pipefail
cd "$(dirname "$0")/.."

all=0
build=1
out=docs/img
while [ $# -gt 0 ]; do
  case $1 in
    --all) all=1 ;;
    --out) out=$2; shift ;;
    --no-build) build=0 ;;
    *) echo "usage: $0 [--all] [--out DIR] [--no-build]" >&2; exit 2 ;;
  esac
  shift
done

source scripts/ios-device.sh
SANDBOX_FEED=data/sandbox/feed.json

# source theme view: one dimension varied per shot, as on the web.
if [ $all = 1 ]; then
  shots=()
  for source in demo sandbox; do
    for theme in light dark; do
      for view in daily lows; do shots+=("$source $theme $view"); done
    done
  done
else
  shots=("demo dark daily" "sandbox dark daily" "sandbox light lows")
fi

if [ $build = 1 ]; then
  scripts/build-ios.sh --sim-only
  xcodegen generate --spec ios/project.yml --quiet
  echo "== building the app"
  xcodebuild build -project ios/Ephemeris.xcodeproj -scheme Ephemeris \
    -destination "id=$udid" -derivedDataPath "$DERIVED" -quiet
fi

log=$(mktemp)
trap 'stop_app; rm -f "$log"; shutdown_device' EXIT
echo "== booting $DEVICE_NAME"
boot_device
xcrun simctl install "$udid" "$APP"
xcrun simctl status_bar "$udid" override --time 9:41 --batteryState charged \
  --batteryLevel 100 --cellularBars 4 --wifiBars 3 --dataNetwork wifi
if [ -f "$SANDBOX_FEED" ]; then
  docs=$(xcrun simctl get_app_container "$udid" "$BUNDLE_ID" data)/Documents
  mkdir -p "$docs"
  cp "$SANDBOX_FEED" "$docs/feed.json"
fi

mkdir -p "$out"
for shot in "${shots[@]}"; do
  read -r source theme view <<<"$shot"
  if [ "$source" = sandbox ] && [ ! -f "$SANDBOX_FEED" ]; then
    echo "skip $shot: no $SANDBOX_FEED"
    continue
  fi
  args=(-EPHEMERIS_VIEW "$view")
  [ "$source" = sandbox ] && args+=(-EPHEMERIS_FEED feed.json)
  xcrun simctl ui "$udid" appearance "$theme"
  launch_and_wait "$log" "${args[@]}"
  # The ready line comes as the report lands, and under load the screen can
  # trail it by seconds. So shoot until two frames 2 s apart are the same:
  # the loading spinner animates, so a loading frame never matches.
  file=$out/ios-$source-$theme-$view.png
  settled=0
  app_screenshot "$file"
  for _ in $(seq 1 30); do
    sleep 2
    app_screenshot "$log.png"
    if cmp -s "$file" "$log.png"; then settled=1; break; fi
    mv "$log.png" "$file"
  done
  rm -f "$log.png"
  if [ $settled = 0 ]; then
    echo "$file: the screen did not settle within a minute" >&2
    exit 1
  fi
  stop_app
  echo "$file: $(grep -o 'EPHEMERIS ready.*' "$log")"
done
xcrun simctl status_bar "$udid" clear
