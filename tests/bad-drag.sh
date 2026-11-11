#!/bin/bash
# End-to-end test of a page that drags badly: a link is dragged, the
# dragstart handler sets no data and no effect, and the drop handler
# moves the link in the page while the drop is still handled. GTK
# crashed on that drop (signal 11) and took gaze with it. On Xvfb the
# crash does not show, only GTK's complaint that leads to it
# (gdk_drop_finish), so the test looks for that line.
#
# Like off-screen.sh it runs the release build in its own X server, D-Bus
# session and HOME, against a test page on localhost.
#
# Needs: Xvfb, xdotool, python3, dbus-run-session.
# Run:   cargo build --release && TMPDIR=/tmp/claude-1000 tests/bad-drag.sh
# Takes about 20 seconds. Exit code 0 when every check passes.

GAZE=$(dirname "$(readlink -f "$0")")/../target/release/gaze
PORT=18767
URL=http://127.0.0.1:$PORT/
for tool in Xvfb xdotool python3 dbus-run-session; do
    command -v $tool >/dev/null || { echo "missing: $tool"; exit 2; }
done
[ -x "$GAZE" ] || { echo "no release build: run cargo build --release"; exit 2; }

T=$(mktemp -d "${TMPDIR:-/tmp}/gaze-test.XXXXXX")
R=$T/run
mkdir -p "$T/home/.gaze" "$R"; chmod 700 "$R"
LOG=$T/home/.gaze/stderr.log

# The top of the page is the thing to drag, the rest is where it lands.
# The title tells how far the drag came.
python3 -c '
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
PAGE = b"""<html><title>page</title><body style="margin:0">
<div class="col" style="height:250px;background:#844"><a id="a" draggable="true" href="/card/1" style="display:block;height:200px;background:#a66">drag me<div>a card</div></a></div>
<div class="col" id="b" style="height:2000px;background:#448">drop here</div>
<script>
let dragged = null;
document.addEventListener("dragstart", e => { dragged = e.target.closest("a"); document.title = "started"; });
document.addEventListener("dragover", e => { if (dragged && e.target.closest(".col")) e.preventDefault(); });
document.addEventListener("drop", async e => {
  const col = e.target.closest(".col");
  if (!dragged || !col) return;
  e.preventDefault();
  col.appendChild(dragged);
  await fetch("/move", { method: "POST", body: "x" });
  document.title = "dropped";
});
</script></body></html>"""
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(PAGE)))
        self.end_headers()
        self.wfile.write(PAGE)
    def do_POST(self):
        self.send_response(204)
        self.end_headers()
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
' $PORT & SRV=$!

n=97; while [ -e /tmp/.X11-unix/X$n ]; do n=$((n+1)); done
export DISPLAY=:$n
Xvfb $DISPLAY -screen 0 1280x800x24 >/dev/null 2>&1 & XV=$!
sleep 1

FAILED=0
check() {   # check <what> <command...>
    if "${@:2}"; then echo "ok    $1"; else echo "FAIL  $1"; FAILED=$((FAILED+1)); fi
}

cleanup() {
    local p v
    for p in /proc/[0-9]*; do
        while IFS= read -r -d '' v; do
            [ "$v" = "XDG_RUNTIME_DIR=$R" ] && { kill "${p#/proc/}" 2>/dev/null; break; }
        done 2>/dev/null < "$p/environ"
    done
    sleep 1.5
    kill $WM $SRV $XV 2>/dev/null
    rm -rf "$T"
}
trap cleanup EXIT

printf 'adblock: false\nhome: about:blank\n' > "$T/home/.gaze/config.yml"
# A drop needs a window manager: GTK finds the window under the pointer
# by the marks a window manager leaves. tile, with an empty HOME so it
# starts nothing of the user's.
WM=
if command -v tile >/dev/null; then
    env -u WAYLAND_DISPLAY HOME="$T/home" XDG_RUNTIME_DIR="$R" tile >/dev/null 2>&1 & WM=$!
    sleep 1
fi
env -u WAYLAND_DISPLAY HOME="$T/home" XDG_RUNTIME_DIR="$R" XDG_DATA_HOME="$T/home/.local/share" \
    XDG_CACHE_HOME="$T/home/.cache" XDG_CONFIG_HOME="$T/home/.config" GDK_BACKEND=x11 \
    GAZE_DEBUG=1 LIBGL_ALWAYS_SOFTWARE=1 GDK_DEBUG=no-portals GTK_A11Y=none GIO_USE_VFS=local \
    dbus-run-session -- "$GAZE" "$URL" > "$T/session.log" 2>&1 &
W=
for _ in $(seq 60); do
    W=$(xdotool search --onlyvisible --name gaze 2>/dev/null); W=${W%%$'\n'*}
    [ -n "$W" ] && break
    sleep 0.5
done
[ -n "$W" ] || { echo "FAIL  gaze did not start"; exit 1; }
GZ=$(xdotool getwindowpid "$W")
sleep 4
alive() { kill -0 "$GZ" 2>/dev/null; }
title() { xdotool getwindowname "$W" 2>/dev/null; }

eval "$(xdotool getwindowgeometry --shell "$W")"
drag() {
    xdotool mousemove $((X + 200)) $((Y + 150)); sleep 0.3
    xdotool mousedown 1; sleep 0.3
    for y in 160 170 180 200 230 260 300 330 370 400 440 470 500 540 570 600 605 610; do xdotool mousemove $((X + 200)) $((Y + y)); sleep 0.2; done
    sleep 1
    xdotool mouseup 1; sleep 1.5
}
drag
check "the drag started" grep -q "started\|dropped" <<< "$(title)"
check "gaze is alive after the drop" alive
check "the page got its drop" grep -q "dropped" <<< "$(title)"
drag
check "gaze is alive after a second drag" alive
quiet() { ! grep -q "gdk_drop_finish" "$LOG" "$T/session.log"; }
check "GTK got one effect for the drop, and did not complain" quiet
[ $FAILED -gt 0 ] && { echo "title: $(title)"; tail -n 15 "$LOG"; tail -n 5 "$T/session.log"; }

[ $FAILED -eq 0 ] && echo "all passed" || echo "$FAILED failed"
exit $FAILED
