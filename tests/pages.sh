#!/bin/bash
# End-to-end test of what gaze does with a page: the reader view, saving
# a PDF, saving the page as HTML, search engines by keyword, the mark on
# a tab that plays sound, a video page (handed to the player, or shown
# as a page for its comments), and a closed tab brought back with its history.
#
# It runs the release build in its own X server (Xvfb), its own D-Bus
# session and a throwaway HOME. Your running gaze and your ~/.gaze are
# never touched, and the test copy reaches only test pages on localhost.
# The sound it plays goes to a sound card that is not there.
#
# Needs: Xvfb, xdotool, python3, dbus-run-session, pdftotext and pdfinfo
# (poppler), import and convert (ImageMagick).
# Run:   cargo build --release && tests/pages.sh
# Takes about two minutes. Exit code 0 when every check passes.
# KEEP=1 keeps the test folder, with a picture of the screen per case.

GAZE=$(dirname "$(readlink -f "$0")")/../target/release/gaze
PORT=18766
URL=http://127.0.0.1:$PORT
for tool in Xvfb xdotool python3 dbus-run-session pdftotext pdfinfo import convert; do
    command -v $tool >/dev/null || { echo "missing: $tool"; exit 2; }
done
[ -x "$GAZE" ] || { echo "no release build: run cargo build --release"; exit 2; }

# A short path on purpose: WebKit puts sockets under the runtime dir, and
# a socket path may be 108 bytes at most.
T=$(mktemp -d "${TMPDIR:-/tmp}/gaze-test.XXXXXX")
R=$T/run
mkdir -p "$T/home/.gaze" "$R"; chmod 700 "$R"
# No ad block list to fetch, and both search engines are the test server:
# nothing leaves the machine.
cat > "$T/home/.gaze/config.yml" <<CONF
adblock: false
dark: false
home: about:blank
search: $URL/search?q=%s
engines:
  w: $URL/wiki?s=%s
downloads: $T/home/Downloads
paper: a4
video_player: $T/player
video_urls:
  - $URL/watch
CONF
# The video player is a stand-in that writes down what it was given.
PLAYED=$T/played.log
printf '#!/bin/sh\necho "$1" >> "%s"\n' "$PLAYED" > "$T/player"; chmod +x "$T/player"; : > "$PLAYED"
printf '</usr/share/alsa/alsa.conf>\npcm.!default { type null }\n' > "$T/asound.conf"
LOG=$T/home/.gaze/stderr.log
ASKED=$T/asked.log
DL=$T/home/Downloads

# The test pages. The article forbids style elements, as strict sites do,
# and has a menu, a side column, a share box and a footer around its text.
python3 -c '
import struct, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
P = "A paragraph of the article, long enough to count as text and not as a caption or a label. "
ARTICLE = ("<html><head><title>The Article</title></head><body>"
  "<nav><a href=/next>MENUWORD home</a> <a href=/next>MENUWORD about</a></nav>"
  "<div><aside>SIDEWORD</aside><article><h1>The Article</h1><div class=share-bar>SHAREWORD</div>"
  "<p>FIRSTWORD " + P + "<a href=/next>a link in the text</a>.</p><p>" + P + "</p><p>" + P + "</p><p>LASTWORD " + P + "</p>"
  "</article></div><footer>FOOTWORD</footer></body></html>")
SOUND = ("<html><head><title>sound</title></head><body>"
  "<button style=\"position:fixed;left:0;top:0;width:100vw;height:100vh\" onclick=\"go()\">play</button><script>"
  "function go(){const c=new AudioContext(),o=c.createOscillator(),g=c.createGain();g.gain.value=0.0001;"
  "o.connect(g);g.connect(c.destination);o.start();document.title=\"playing\";}</script></body></html>")
# A page with a folded part, a hidden part, a part its script builds and
# a shadow tree. Its body has a handler that would rename a saved copy.
FOLDED = ("<html><head><title>Folded</title><meta http-equiv=refresh content=\"900;url=/next\"></head>"
  "<body onload=\"if(location.protocol==\x27file:\x27)document.title=\x27HANDLER\x27\">"
  "<details><summary>More</summary><p>FOLDWORD</p></details><div style=\"display:none\">UNSEENWORD</div>"
  "<a href=next>a link</a><my-box></my-box>"
  "<script>/* SCRIPTWORD */ document.body.append(\"BUILTWORD\");"
  "document.querySelector(\"my-box\").attachShadow({mode:\"open\"}).innerHTML=\"<p>SHADOWWORD</p>\";</script>"
  "</body></html>")
# A list with one video, a link to one comment, and a page that starts a
# silent video by itself and tells the server how that went. The video
# page is that last page too. It has to be a video element: WebKit lets a
# silent audio element start even where nothing may start by itself.
VIDEOS = "<html><head><title>videos</title></head><body><a href=\"/watch?v=one\">a video</a></body></html>"
COMMENT = "<html><head><title>comment</title></head><body><a href=\"/watch?v=two&amp;lc=abc\">a comment</a></body></html>"
CLIP = ("<html><head><title>clip</title></head><body><video id=a muted loop src=/silence.wav></video><script>"
  "document.getElementById(\"a\").play().then(function(){fetch(\"/started\"+location.pathname)},"
  "function(){fetch(\"/refused\"+location.pathname)});</script></body></html>")
WAV = (b"RIFF" + struct.pack("<I", 36 + 8000) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, 8000, 8000, 1, 8)
  + b"data" + struct.pack("<I", 8000) + bytes([128]) * 8000)
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        open(sys.argv[2], "a").write(self.path + "\n")
        page = self.path.split("?")[0]
        b = ARTICLE if page == "/article" else SOUND if page == "/sound" else FOLDED if page == "/folded" else VIDEOS if page == "/videos" else COMMENT if page == "/comment" else CLIP if page in ("/clip", "/watch") else "<html><title>%s</title><body>short</body></html>" % page.strip("/")
        b = WAV if page == "/silence.wav" else b.encode()
        self.send_response(200)
        self.send_header("Content-Type", "audio/wav" if page == "/silence.wav" else "text/html; charset=utf-8")
        if page == "/article": self.send_header("Content-Security-Policy", "default-src \x27self\x27; style-src \x27self\x27")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
' $PORT "$ASKED" & SRV=$!

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
title_is() { [ "$(xdotool getwindowname "$W" 2>/dev/null)" = "$1 - gaze" ]; }
said() { grep -q "says $1" "$LOG"; }
said_times() { [ "$(grep -c "says $2" "$LOG")" -ge "$1" ]; }
dialog() { xdotool search --onlyvisible --name '^Print$' >/dev/null 2>&1; }
# The middle of the screen is mostly dark.
dark_now() { [ "$(import -window root png:- 2>/dev/null | convert - -gravity center -crop 60%x60%+0+0 -format '%[fx:mean<0.35?1:0]' info: 2>/dev/null)" = 1 ]; }
pdf_has() { pdftotext "$1" - 2>/dev/null | grep -q "$2"; }
asked() { grep -qxF "$1" "$ASKED"; }
asked_times() { [ "$(grep -cxF "$2" "$ASKED")" -ge "$1" ]; }
played() { grep -qxF "$URL$1" "$PLAYED"; }
# Wait up to ten seconds for a command to come true.
wait_for() { for _ in $(seq 40); do "$@" && return 0; sleep 0.25; done; return 1; }

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
cleanup() { stop; kill $SRV $XV 2>/dev/null; [ -n "$KEEP" ] || rm -rf "$T"; }
trap cleanup EXIT

start() {   # start <case name> <url>
    CASE=$1; N=$((N+1))
    : > "$LOG"
    env -u WAYLAND_DISPLAY -u PULSE_SERVER LANGUAGE=en HOME="$T/home" XDG_RUNTIME_DIR="$R" XDG_DATA_HOME="$T/home/.local/share" \
        XDG_CACHE_HOME="$T/home/.cache" XDG_CONFIG_HOME="$T/home/.config" GDK_BACKEND=x11 \
        GAZE_DEBUG=1 LIBGL_ALWAYS_SOFTWARE=1 ${ONLY:+GAZE_CPU=1 GSK_RENDERER=cairo} GDK_DEBUG=no-portals GTK_A11Y=none GIO_USE_VFS=local \
        ALSA_CONFIG_PATH="$T/asound.conf" \
        dbus-run-session -- "$GAZE" "$2" > "$T/session.log" 2>&1 &
    W=
    for _ in $(seq 60); do
        W=$(xdotool search --onlyvisible --name gaze 2>/dev/null); W=${W%%$'\n'*}
        [ -n "$W" ] && break
        sleep 0.5
    done
    [ -n "$W" ] || { fail "gaze did not start"; return 1; }
    wait_for title_is "$3" || { fail "the page did not load"; return 1; }
    sleep 1
    xdotool windowfocus "$W"
}
shot() { [ -n "$KEEP" ] && sleep 1 && import -window root "$T/$1.png" 2>/dev/null; }

start "reader view" "$URL/article" "The Article" && {
    xdotool key g r
    check "gr turns the reader view on" wait_for said "Reader view"
    shot reader
    xdotool key ctrl+shift+p
    check "the page is saved under its title" wait_for test -s "$DL/The Article.pdf"
    check "on the paper named in the config" eval 'pdfinfo "$DL/The Article.pdf" | grep -q "(A4)"'
    check "the PDF has the article, first word to last" eval 'pdf_has "$DL/The Article.pdf" FIRSTWORD && pdf_has "$DL/The Article.pdf" LASTWORD'
    for word in MENUWORD SIDEWORD SHAREWORD FOOTWORD; do
        check "the PDF has no $word" not pdf_has "$DL/The Article.pdf" $word
    done
    xdotool key g r
    check "gr again goes back to the page" wait_for said "Back to the page"
    xdotool key ctrl+shift+p
    check "a second PDF gets a name of its own" wait_for test -s "$DL/The Article-2.pdf"
    check "the page as it was has its menu" pdf_has "$DL/The Article-2.pdf" MENUWORD
    xdotool key g r
    wait_for said_times 2 "Reader view"; sleep 0.5
    # The menu is gone, so the one link left is the one in the text.
    xdotool key f; sleep 0.7; xdotool key a
    check "a link in the article is followed by its hint" wait_for title_is next
    xdotool key g r
    check "a page with no article says so" wait_for said "No article found"
}; stop

start "print" "$URL/article" "The Article" && {
    xdotool key ctrl+p
    check "Ctrl-p brings the print dialog" wait_for dialog
    shot print
    # No window manager here, so the dialog has to be given the keys.
    xdotool windowfocus "$(xdotool search --onlyvisible --name '^Print$' | head -1)"; sleep 0.5
    xdotool key Escape
    check "Escape closes it" wait_for not dialog
    xdotool windowfocus "$W"; sleep 0.5; xdotool key g r
    check "and the keys are gaze's again" wait_for said "Reader view"
}; stop

start "a dark page on paper" "$URL/article" "The Article" && {
    # gaze turns this light page dark, though the page forbids style
    # elements. On paper the turn gave black sheets with no text in them,
    # so the turn is for the screen alone.
    xdotool key D
    wait_for said "Dark pages on"
    check "D turns the page dark" wait_for dark_now
    shot dark
    xdotool key ctrl+shift+p
    check "the dark page is saved" wait_for test -s "$DL/The Article-3.pdf"
    check "and the PDF has its text" pdf_has "$DL/The Article-3.pdf" FIRSTWORD
    xdotool key g r; wait_for said "Reader view"
    check "the reader view is dark too" wait_for dark_now
    shot dark-reader
    xdotool key ctrl+shift+p
    check "the reader view of it is saved" wait_for test -s "$DL/The Article-4.pdf"
    check "with its text and no menu" eval 'pdf_has "$DL/The Article-4.pdf" LASTWORD && ! pdf_has "$DL/The Article-4.pdf" MENUWORD'
    xdotool key g r; wait_for said "Back to the page"
    xdotool key D
    wait_for said "Dark pages off"
    check "D again gives the light page back" wait_for not dark_now
}; stop

start "save as HTML" "$URL/folded" "Folded" && {
    xdotool key ctrl+s
    check "Ctrl-s saves the page under its title" wait_for said "Saved $DL/Folded.html"
    for word in FOLDWORD UNSEENWORD BUILTWORD SHADOWWORD; do
        check "the file has $word" grep -q $word "$DL/Folded.html"
    done
    check "and none of the page's scripts" not grep -q 'SCRIPTWORD\|<script' "$DL/Folded.html"
    check "its links are the site's" grep -q "<base href=\"$URL/folded\">" "$DL/Folded.html"
    check "nothing in it sends the reader on" not grep -qi refresh "$DL/Folded.html"
    xdotool key ctrl+s
    check "a second file gets a name of its own" wait_for test -s "$DL/Folded-2.html"
}; stop

start "the saved file" "file://$DL/Folded.html" "Folded" && {
    check "none of the page's code runs in it" title_is Folded
    xdotool key ctrl+shift+p
    check "it is saved as a PDF" wait_for test -s "$DL/Folded.pdf"
    check "the folded part shows in it, and the shadow tree" eval 'pdf_has "$DL/Folded.pdf" FOLDWORD && pdf_has "$DL/Folded.pdf" SHADOWWORD'
    check "what the page hid stays hidden" not pdf_has "$DL/Folded.pdf" UNSEENWORD
}; stop

start "search engines" "$URL/start" "start" && {
    xdotool key o; sleep 0.5; xdotool type --delay 30 "w free will"; xdotool key Return
    check "a keyword first asks that engine" wait_for grep -qx "/wiki?s=free+will" "$ASKED"
    xdotool key o; sleep 0.5; xdotool type --delay 30 "free will"; xdotool key Return
    check "no keyword asks the usual one" wait_for grep -qx "/search?q=free+will" "$ASKED"
}; stop

start "sound" "$URL/sound" "sound" && {
    xdotool mousemove --window "$W" 600 400 click 1
    wait_for title_is playing
    check "a tab that plays sound is noticed" wait_for grep -q "a tab plays sound" "$LOG"
    shot sound
    xdotool key Escape; sleep 0.3; xdotool key alt+m
    check "Alt-m turns its sound off" wait_for said "Sound off for the tab"
    shot muted
    xdotool key alt+m
    check "and on again" wait_for said "Sound on for the tab"
}; stop

start "a video page" "$URL/videos" "videos" && {
    xdotool key f; sleep 0.7; xdotool key a
    check "a link to a video goes to the player" wait_for played "/watch?v=one"
    sleep 1
    check "and gaze does not load its page" not asked "/watch?v=one"
    xdotool key V
    check "V opens the video's own page in gaze" wait_for asked "/watch?v=one"
    check "where nothing starts by itself" wait_for asked "/refused/watch"
    check "and the player is not started again" [ "$(wc -l < "$PLAYED")" = 1 ]
    shot video-page
    xdotool key d; sleep 0.5; xdotool key u
    # It comes back with its history, from the cache: the title shows it.
    check "the closed page comes back as a page" wait_for title_is clip
    check "with the player still left alone" [ "$(wc -l < "$PLAYED")" = 1 ]
}; stop

start "a page that plays by itself" "$URL/clip" "clip" && {
    check "any other page may start a silent video" wait_for asked "/started/clip"
}; stop

: > "$PLAYED"
start "a link to a comment" "$URL/comment" "comment" && {
    xdotool key f; sleep 0.7; xdotool key a
    check "a link to one comment opens in gaze" wait_for asked "/watch?v=two&lc=abc"
    sleep 1
    check "and the player is left alone" not test -s "$PLAYED"
}; stop

start "a closed tab keeps its history" "$URL/article" "The Article" && {
    xdotool key f; sleep 0.7; xdotool key a
    wait_for title_is next
    xdotool key d; sleep 1; xdotool key u
    check "u brings the tab back on its page" wait_for title_is next
    xdotool key H
    check "and back goes to the page before it" wait_for title_is "The Article"
    xdotool key L
    check "and forward again" wait_for title_is next
}; stop

if [ $FAILED = 0 ]; then echo "all passed"; else echo "$FAILED failed"; fi
[ -n "$KEEP" ] && echo "kept: $T"
exit $FAILED
