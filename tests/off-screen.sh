#!/bin/bash
# End-to-end test of pages stopping while gaze is off screen: the web
# processes stop `freeze` seconds after the window is unmapped (as tile
# does on another workspace), go on when it is mapped again, and never
# stop with freeze: 0.
#
# Like login-box.sh it runs the release build in its own X server, D-Bus
# session and HOME, against a test page on localhost.
#
# Needs: Xvfb, xdotool, python3, dbus-run-session.
# Run:   cargo build --release && TMPDIR=/tmp/claude-1000 tests/off-screen.sh
# Takes about 40 seconds. Exit code 0 when every check passes.

GAZE=$(dirname "$(readlink -f "$0")")/../target/release/gaze
PORT=18766
URL=http://127.0.0.1:$PORT/
for tool in Xvfb xdotool python3 dbus-run-session; do
    command -v $tool >/dev/null || { echo "missing: $tool"; exit 2; }
done
[ -x "$GAZE" ] || { echo "no release build: run cargo build --release"; exit 2; }

T=$(mktemp -d "${TMPDIR:-/tmp}/gaze-test.XXXXXX")
R=$T/run
mkdir -p "$T/home/.gaze" "$R"; chmod 700 "$R"
LOG=$T/home/.gaze/stderr.log

# The test page counts in its title ten times a second, so it keeps its
# web process busy while it runs.
python3 -c '
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
PAGE = b"<html><title>0</title><body><script>let n = 0; setInterval(() => document.title = String(++n), 100);</script></body></html>"
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(PAGE)))
        self.end_headers()
        self.wfile.write(PAGE)
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
' $PORT & SRV=$!

n=97; while [ -e /tmp/.X11-unix/X$n ]; do n=$((n+1)); done
export DISPLAY=:$n
Xvfb $DISPLAY -screen 0 1280x800x24 >/dev/null 2>&1 & XV=$!
sleep 1

N=0; FAILED=0; CASE=; W=; GZ=
fail() { echo "FAIL  $N $CASE: $1"; FAILED=$((FAILED+1)); }
check() {   # check <what> <command...>
    if "${@:2}"; then echo "ok    $N $CASE: $1"; else fail "$1"; fi
}

# The test gaze's web processes: those with it up their parent chain.
webprocs() {
    local p c q
    for p in /proc/[0-9]*; do
        { read -r c < "$p/comm"; } 2>/dev/null || continue
        [ "$c" = WebKitWebProces ] || continue
        q=${p#/proc/}
        while [ "$q" -gt 1 ] 2>/dev/null; do
            [ "$q" = "$GZ" ] && { echo "${p#/proc/}"; break; }
            q=$(awk '{print $4}' "/proc/$q/stat" 2>/dev/null) || break
        done
    done
}
# State letter of each: T is stopped.
stopped() { local p s; for p in $(webprocs); do s=$(awk '{print $3}' /proc/$p/stat); [ "$s" = T ] || return 1; done; [ -n "$(webprocs)" ]; }
running() { local p s; for p in $(webprocs); do s=$(awk '{print $3}' /proc/$p/stat); [ "$s" = T ] && return 1; done; [ -n "$(webprocs)" ]; }
counting() { local a b; a=$(xdotool getwindowname "$W"); sleep 1; b=$(xdotool getwindowname "$W"); [ "$a" != "$b" ]; }

stop() {
    local p v
    for p in /proc/[0-9]*; do
        while IFS= read -r -d '' v; do
            [ "$v" = "XDG_RUNTIME_DIR=$R" ] && { kill -CONT "${p#/proc/}" 2>/dev/null; kill "${p#/proc/}" 2>/dev/null; break; }
        done 2>/dev/null < "$p/environ"
    done
    sleep 1.5
    rm -f "$T/home/.gaze/session.json"
}
cleanup() { stop; kill $SRV $XV 2>/dev/null; rm -rf "$T"; }
trap cleanup EXIT

start() {   # start <case name> <freeze seconds>
    CASE=$1; N=$((N+1))
    printf 'adblock: false\nhome: about:blank\nfreeze: %s\n' "$2" > "$T/home/.gaze/config.yml"
    : > "$LOG"
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
    [ -n "$W" ] || { fail "gaze did not start"; return 1; }
    GZ=$(xdotool getwindowpid "$W")
    sleep 4     # the page loads and starts counting
}

start "freeze: 3" 3 && {
    check "the page runs on screen" counting
    xdotool windowunmap "$W"; sleep 1
    check "it runs on just after the window leaves" running
    sleep 4
    check "its web processes stop after 3 s off screen" stopped
    check "gaze says so" grep -q "web processes stopped" "$LOG"
    xdotool windowmap "$W"; sleep 1
    [ $FAILED -gt 0 ] && grep -i "screen\|stopped" "$LOG"
    check "they go on when the window is back" running
    check "the page counts again" counting
}; stop

start "freeze: 0" 0 && {
    xdotool windowunmap "$W"; sleep 5
    check "nothing stops with freeze: 0" running
}; stop

[ $FAILED -eq 0 ] && echo "all passed" || echo "$FAILED failed"
exit $FAILED
