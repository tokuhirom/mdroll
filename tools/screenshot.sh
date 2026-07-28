#!/usr/bin/env bash
# Render a Markdown file in a real terminal and save a PNG of the result.
#
# Runs kitty on a headless X server, so it works over SSH and in CI. Note that
# kitty does not implement DECDHL, so double-height headings do not appear in
# screenshots taken this way.
#
#   tools/screenshot.sh <input.md> <output.png> [mdroll args...]
set -euo pipefail

input=${1:?usage: screenshot.sh <input.md> <output.png> [args...]}
output=${2:?usage: screenshot.sh <input.md> <output.png> [args...]}
shift 2

display=${MDROLL_SHOT_DISPLAY:-:99}
width=${MDROLL_SHOT_WIDTH:-1000}
height=${MDROLL_SHOT_HEIGHT:-660}
font=${MDROLL_SHOT_FONT:-DejaVu Sans Mono}
size=${MDROLL_SHOT_FONT_SIZE:-15}
background=${MDROLL_SHOT_BG:-#282a36}
binary=${MDROLL_BIN:-./target/release/mdroll}

cleanup() {
    [[ -n ${kitty_pid:-} ]] && kill "$kitty_pid" 2>/dev/null || true
    [[ -n ${xvfb_pid:-} ]] && kill "$xvfb_pid" 2>/dev/null || true
}
trap cleanup EXIT

if ! DISPLAY=$display xdpyinfo >/dev/null 2>&1; then
    Xvfb "$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/dev/null 2>&1 &
    xvfb_pid=$!
    sleep 2
fi

DISPLAY=$display kitty --config NONE \
    -o "font_family=$font" \
    -o "font_size=$size" \
    -o "background=$background" \
    -o "initial_window_width=$width" \
    -o "initial_window_height=$height" \
    -o remember_window_size=no \
    -o cursor_blink_interval=0 \
    -- "$binary" "$input" "$@" >/dev/null 2>&1 &
kitty_pid=$!

sleep "${MDROLL_SHOT_SETTLE:-5}"
DISPLAY=$display import -window root "$output"
echo "wrote $output"
