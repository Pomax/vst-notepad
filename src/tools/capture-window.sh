#!/usr/bin/env bash
#
# Screenshot the real editor window on macOS.
#
# Usage: capture-window.sh [--theme light|dark|auto] [--out FILE] [--exe PATH]
#
# Requires Screen Recording permission (screencapture) and Accessibility
# permission (System Events) for whichever terminal runs it.

set -euo pipefail

theme=auto
out=target/window.png
exe=target/debug/examples/preview
settle=2.5

while [ $# -gt 0 ]; do
    case "$1" in
        --theme) theme=$2; shift 2 ;;
        --out)   out=$2;   shift 2 ;;
        --exe)   exe=$2;   shift 2 ;;
        --settle) settle=$2; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

if [ ! -x "$exe" ]; then
    echo "$exe not found - run: cargo build -p notepad-plugin --example preview" >&2
    exit 1
fi

mkdir -p "$(dirname "$out")"

"$exe" "$theme" &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' EXIT

bounds=""
for _ in $(seq 1 60); do
    bounds=$(osascript -e "
        tell application \"System Events\"
            tell (first process whose unix id is $pid)
                get {position, size} of front window
            end tell
        end tell" 2>/dev/null) || bounds=""
    [ -n "$bounds" ] && break
    sleep 0.5
done

if [ -z "$bounds" ]; then
    echo "the preview window never appeared" >&2
    exit 1
fi

IFS=', ' read -r x y w h <<< "$bounds"

# Let the GL context draw a few frames before grabbing pixels.
sleep "$settle"
screencapture -x -R "${x},${y},${w},${h}" "$out"

echo "captured ${w}x${h} -> $out"
