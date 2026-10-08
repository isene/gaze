#!/bin/bash
# End-to-end test of a site's own password dialog (HTTP Basic) in gaze:
# the master password prompt, the keys, and the offer to save.
#
# It runs the release build in its own X server (Xvfb), its own D-Bus
# session and a throwaway HOME. Your running gaze and your ~/.gaze are
# never touched, and the test copy reaches only a test page on localhost.
#
# Needs: Xvfb, xdotool, python3, dbus-run-session.
# Run:   cargo build --release && tests/login-box.sh
# Takes about two minutes. Exit code 0 when every check passes.

GAZE=$(dirname "$(readlink -f "$0")")/../target/release/gaze
PORT=18765
URL=http://127.0.0.1:$PORT/
for tool in Xvfb xdotool python3 dbus-run-session; do
    command -v $tool >/dev/null || { echo "missing: $tool"; exit 2; }
done
[ -x "$GAZE" ] || { echo "no release build: run cargo build --release"; exit 2; }

# A short path on purpose: WebKit puts sockets under the runtime dir, and
# a socket path may be 108 bytes at most.
T=$(mktemp -d "${TMPDIR:-/tmp}/gaze-test.XXXXXX")
R=$T/run
mkdir -p "$T/home/.gaze" "$R"; chmod 700 "$R"
# No ad block list to fetch and no search engine: nothing leaves the machine.
printf 'adblock: false\nhome: about:blank\nsearch: %s?q=%%s\n' "$URL" > "$T/home/.gaze/config.yml"
LOG=$T/home/.gaze/stderr.log
STORE=$T/home/.gaze/sync/passwords

# The test page: "in" with the right login, a 401 without.
python3 -c '
import sys, base64
from http.server import BaseHTTPRequestHandler, HTTPServer
OK = "Basic " + base64.b64encode(b"tester:secret-pw").decode()
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get("Authorization") != OK:
            b = b"<html><title>out</title><body>401 no login</body></html>"
            self.send_response(401)
            self.send_header("WWW-Authenticate", "Basic realm=\"Test\"")
        else:
            b = b"<html><title>in</title><body>logged in</body></html>"
            self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
' $PORT & SRV=$!

n=97; while [ -e /tmp/.X11-unix/X$n ]; do n=$((n+1)); done
export DISPLAY=:$n
Xvfb $DISPLAY -screen 0 1280x800x24 >/dev/null 2>&1 & XV=$!
sleep 1

N=0; FAILED=0; CASE=; W=
fail() { echo "FAIL  $N $CASE: $1"; FAILED=$((FAILED+1)); }
check() {   # check <what> <command...>
    if "${@:2}"; then echo "ok    $N $CASE: $1"; else fail "$1"; fi
}
not() { ! "$@"; }
logged_in() { [ "$(xdotool getwindowname "$W" 2>/dev/null)" = "in - gaze" ]; }
offered() { grep -q "offer to save tester for $1" "$LOG"; }
type_login() {
    xdotool type --delay 40 tester; xdotool key Tab
    xdotool type --delay 40 secret-pw; xdotool key Return
    sleep 3
}

stop() {
    # Everything the test session started has the test runtime dir in its
    # environment: gaze, its D-Bus daemon and what that daemon started.
    local p v
    for p in /proc/[0-9]*; do
        while IFS= read -r -d '' v; do
            [ "$v" = "XDG_RUNTIME_DIR=$R" ] && { kill "${p#/proc/}" 2>/dev/null; break; }
        done 2>/dev/null < "$p/environ"
    done
    sleep 1.5
    rm -f "$T/home/.gaze/session.json"     # each case starts with one tab
}
cleanup() { stop; kill $SRV $XV 2>/dev/null; rm -rf "$T"; }
trap cleanup EXIT

start() {   # start <case name> <url>
    CASE=$1; N=$((N+1))
    : > "$LOG"
    env -u WAYLAND_DISPLAY HOME="$T/home" XDG_RUNTIME_DIR="$R" XDG_DATA_HOME="$T/home/.local/share" \
        XDG_CACHE_HOME="$T/home/.cache" XDG_CONFIG_HOME="$T/home/.config" GDK_BACKEND=x11 \
        GAZE_DEBUG=1 LIBGL_ALWAYS_SOFTWARE=1 GDK_DEBUG=no-portals GTK_A11Y=none GIO_USE_VFS=local \
        dbus-run-session -- "$GAZE" "$2" > "$T/session.log" 2>&1 &
    W=
    for _ in $(seq 60); do
        W=$(xdotool search --onlyvisible --name gaze 2>/dev/null); W=${W%%$'\n'*}
        [ -n "$W" ] && break
        sleep 0.5
    done
    [ -n "$W" ] || { fail "gaze did not start"; return 1; }
    sleep 4     # the page asks, and the prompt or the dialog comes up
    xdotool windowfocus "$W"
}

start "no store yet" "$URL" && {
    type_login
    check "keys go straight into the dialog, Tab reaches the password" logged_in
    check "offer to save the typed login" offered http://127.0.0.1
    xdotool key y; sleep 1; xdotool type --delay 40 m-pass; xdotool key Return; sleep 2
    check "the login is saved" test -s "$STORE"
}; stop

start "locked store" "$URL" && {
    xdotool type --delay 40 m-pass; xdotool key Return; sleep 3
    check "the master password answers the dialog" logged_in
    check "no offer to save a login the store has" not offered http://127.0.0.1
}; stop

start "locked store, Escape at the master prompt" "$URL" && {
    xdotool key Escape; sleep 3
    type_login
    check "the dialog comes and the typed login gets in" logged_in
    check "offer to save the typed login" offered http://127.0.0.1
    xdotool key n; sleep 1
}; stop

start "locked store, wrong master password" "$URL" && {
    xdotool type --delay 40 wrong; xdotool key Return; sleep 3
    type_login
    check "the dialog comes and the typed login gets in" logged_in
}; stop

start "Escape in the dialog" "$URL" && {
    xdotool key Escape; sleep 3     # past the master prompt, to the dialog
    xdotool key Escape; sleep 2     # out of the dialog
    xdotool key o; sleep 1; xdotool key x; sleep 0.5; xdotool key Escape
    check "the dialog closes and the keys are gaze's again" grep -q '"x") in Command' "$LOG"
    check "nobody got in" not logged_in
}; stop

start "open store without this site" "http://localhost:$PORT/" && {
    size=$(stat -c %s "$STORE")
    xdotool type --delay 40 m-pass; xdotool key Return; sleep 3
    type_login
    check "the dialog comes and the typed login gets in" logged_in
    check "offer to save the typed login" offered http://localhost
    xdotool key y; sleep 2
    check "the login is saved" test "$(stat -c %s "$STORE")" -gt "$size"
}; stop

start "private tab" about:blank && {
    xdotool key colon; sleep 0.5; xdotool type --delay 40 "private $URL"; xdotool key Return; sleep 4
    type_login
    check "the same keys, and no master prompt in the way" logged_in
    check "no offer to save" not offered http://127.0.0.1
}; stop

echo
if [ "$FAILED" = 0 ]; then echo "all passed"; else echo "$FAILED failed"; fi
exit $((FAILED > 0))
