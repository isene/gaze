#!/bin/bash
# End-to-end test of a sign-in that asks for the account and the password
# on two steps, as Google does: each account keeps its own password, and
# the fill picks the account the page is for.
#
# It runs the release build in its own X server (Xvfb), its own D-Bus
# session and a throwaway HOME. Your running gaze and your ~/.gaze are
# never touched, and the test copy reaches only a test page on localhost.
#
# Needs: Xvfb, xdotool, python3, dbus-run-session.
# Run:   cargo build --release && tests/two-step.sh
# Takes about a minute. Exit code 0 when every check passes.

GAZE=$(dirname "$(readlink -f "$0")")/../target/release/gaze
PORT=18767
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

# The test page. Step one asks for the account, step two for the password.
# With ?pick=<account> it starts on step two, which names the account in a
# hidden field only, like Google's page after "Choose an account".
mkdir -p "$T/www"
cat > "$T/www/index.html" <<'HTML'
<html><head><title>step one</title></head><body>
<form id="f"><input id="u" type="email" name="identifier" autocomplete="username webauthn" autofocus>
<button>Next</button></form>
<script>
const f = document.getElementById('f');
const two = who => {
  f.innerHTML = (who ? '<input type="email" autocomplete="username" hidden value="' + who + '">' : '') +
    '<input id="p" type="password" name="Passwd"><button>Next</button>';
  document.title = 'step two';
  document.getElementById('p').focus();
  f.onsubmit = e => { e.preventDefault(); document.title = 'sent'; };
};
const pick = new URLSearchParams(location.search).get('pick');
if (pick) two(pick);
else f.onsubmit = e => { e.preventDefault(); two(''); };
</script></body></html>
HTML
python3 -m http.server $PORT -b 127.0.0.1 -d "$T/www" >/dev/null 2>&1 & SRV=$!

n=97; while [ -e /tmp/.X11-unix/X$n ]; do n=$((n+1)); done
export DISPLAY=:$n
Xvfb $DISPLAY -screen 0 1280x800x24 >/dev/null 2>&1 & XV=$!
sleep 1

N=0; FAILED=0; CASE=; W=
fail() { echo "FAIL  $N $CASE: $1"; FAILED=$((FAILED+1)); }
check() {   # check <what> <command...>
    if "${@:2}"; then echo "ok    $N $CASE: $1"; else fail "$1"; fi
}
offered() { grep -q "offer to save $1 for http://127.0.0.1" "$LOG"; }
says() { grep -q "says $1" "$LOG"; }
master() { xdotool type --delay 40 m-pass; xdotool key Return; sleep 2; }

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
    sleep 3
    xdotool windowfocus "$W"; sleep 0.5
}

start "account typed on step one" "$URL" && {
    xdotool type --delay 30 alice@example.com; xdotool key Return; sleep 1
    xdotool type --delay 30 pw-alice; xdotool key Return; sleep 1
    check "the password is offered for the account of step one" offered alice@example.com
    xdotool key y; sleep 1; master
    check "the login is saved" test -s "$STORE"
}; stop

start "account in a hidden field" "$URL?pick=bob@example.com" && {
    xdotool type --delay 30 pw-bob; xdotool key Return; sleep 1
    check "the password is offered for the hidden account" offered bob@example.com
    xdotool key y; sleep 1; master
}; stop

start "fill by the hidden field" "$URL?pick=alice@example.com" && {
    xdotool key Escape; sleep 0.3; xdotool key g p; sleep 1; master
    check "alice's page gets alice's password, though bob's is newer" says "Filled alice@example.com"
}; stop

start "fill by the account of step one" "$URL" && {
    xdotool type --delay 30 bob@example.com; xdotool key Return; sleep 1
    xdotool key Escape; sleep 0.3; xdotool key g p; sleep 1; master
    check "the account typed on step one gets its password" says "Filled bob@example.com"
}; stop

echo
if [ "$FAILED" = 0 ]; then echo "all passed"; else echo "$FAILED failed"; fi
exit $((FAILED > 0))
