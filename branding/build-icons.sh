#!/bin/bash
# Build every icon NetPaste needs from the two SVG masters.
#
#   icon.svg        the full mark, used at 128px and above
#   icon-small.svg  a simplified mark for 64px and below, where the link
#                   nodes of the full icon collapse into noise
#
# Needs macOS (sips, iconutil) and Google Chrome for the SVG -> PNG step.
set -euo pipefail
cd "$(dirname "$0")"

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
OUT=icons
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
rm -rf "$OUT" && mkdir -p "$OUT"

render() { # render <svg> <png>
  cat > "$TMP/page.html" <<HTML
<!doctype html><meta charset="utf-8">
<style>html,body{margin:0;padding:0;background:transparent}img{display:block}</style>
<img src="$PWD/$1" width="1024" height="1024">
HTML
  # Chrome writes the screenshot but does not reliably exit, so run it in
  # the background and stop it once the file appears.
  "$CHROME" --headless=new --disable-gpu --no-first-run --hide-scrollbars \
    --user-data-dir="$TMP/profile-$2" --default-background-color=00000000 \
    --window-size=1024,1024 --screenshot="$TMP/$2" "file://$TMP/page.html" \
    >/dev/null 2>&1 &
  chrome_pid=$!
  for _ in $(seq 1 30); do
    [ -s "$TMP/$2" ] && break
    sleep 1
  done
  sleep 1
  kill "$chrome_pid" 2>/dev/null || true
  wait "$chrome_pid" 2>/dev/null || true
  [ -s "$TMP/$2" ] || { echo "failed to render $1"; exit 1; }
}

echo "rendering masters…"
render icon.svg master-full.png
render icon-small.svg master-small.png

# Which master to use at which size: the simplified one stays readable small.
master_for() { [ "$1" -le 64 ] && echo "$TMP/master-small.png" || echo "$TMP/master-full.png"; }

echo "sizing…"
for s in 16 32 48 64 128 256 512 1024; do
  sips -Z $s "$(master_for $s)" --out "$OUT/icon-$s.png" >/dev/null
done

echo "macOS .icns…"
SET="$TMP/AppIcon.iconset"
mkdir -p "$SET"
cp "$OUT/icon-16.png"   "$SET/icon_16x16.png"
cp "$OUT/icon-32.png"   "$SET/icon_16x16@2x.png"
cp "$OUT/icon-32.png"   "$SET/icon_32x32.png"
cp "$OUT/icon-64.png"   "$SET/icon_32x32@2x.png"
cp "$OUT/icon-128.png"  "$SET/icon_128x128.png"
cp "$OUT/icon-256.png"  "$SET/icon_128x128@2x.png"
cp "$OUT/icon-256.png"  "$SET/icon_256x256.png"
cp "$OUT/icon-512.png"  "$SET/icon_256x256@2x.png"
cp "$OUT/icon-512.png"  "$SET/icon_512x512.png"
cp "$OUT/icon-1024.png" "$SET/icon_512x512@2x.png"
iconutil -c icns "$SET" -o "$OUT/AppIcon.icns"

echo "Windows .ico…"
python3 make-ico.py "$OUT/NetPaste.ico" \
  "$OUT/icon-16.png" "$OUT/icon-32.png" "$OUT/icon-48.png" \
  "$OUT/icon-64.png" "$OUT/icon-128.png" "$OUT/icon-256.png"

echo
ls -lh "$OUT" | sed 's/^/  /'
