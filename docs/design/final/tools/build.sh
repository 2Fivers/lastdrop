#!/bin/sh
# build.sh <readout|flyout|both>: generate the final mocks, measure page height, render PNGs.
S="${TMPDIR:-/tmp}/lastdrop-final"
F="F:/Homelab/Sites/2Fivers/lastdrop/docs/design/final"
FW='F:\Homelab\Sites\2Fivers\lastdrop\docs\design\final'
EDGE="/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
cd "$S" || exit 1
height() {
  # page height: inject a measuring script into a temp copy
  sed 's#</body>#<script>document.title="H:"+Math.ceil(document.documentElement.scrollHeight);</script></body>#' "$F/$1.html" > "$F/_m.html"
  "$EDGE" --headless=new --disable-gpu --window-size=1200,900 --virtual-time-budget=1500 --dump-dom "file:///$F/_m.html" 2>/dev/null | grep -o '<title>H:[0-9]*' | sed 's/.*H://'
  rm -f "$F/_m.html"
}
shot() { # name scale outfile height
  "$EDGE" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=$2 --window-size=1200,$4 --virtual-time-budget=1500 --screenshot="$FW\\$3" "file:///$F/$1.html" >/dev/null 2>&1
}
if [ "$1" = "readout" ] || [ "$1" = "both" ]; then
  python -I gen_readout.py "$F/readout.html" || exit 1
  H=$(height readout); echo "readout height $H"
  shot readout 1 readout-100.png $H
  shot readout 1.5 readout-150.png $H
fi
if [ "$1" = "flyout" ] || [ "$1" = "both" ]; then
  python -I gen_flyout.py "$F/flyout.html" || exit 1
  H=$(height flyout); echo "flyout height $H"
  shot flyout 1 flyout-100.png $H
  shot flyout 1.5 flyout-150.png $H
fi
ls -la "$F"
