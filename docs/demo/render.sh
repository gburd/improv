#!/usr/bin/env bash
# Render docs/demo/NAME.cast to docs/src/img/NAME.gif and NAME.mp4, in the
# one theme every console movie uses, and NAME.png, the MP4's poster: the
# frame at SECONDS into the movie. Run from the repo root:
#   docs/demo/render.sh NAME SECONDS
# Needs agg, gifsicle, ffmpeg and ImageMagick. Pauses longer than 3 s are
# cut to 3 s.
set -euo pipefail
name=${1:?usage: docs/demo/render.sh NAME SECONDS}
at=${2:?usage: docs/demo/render.sh NAME SECONDS}
T=$(mktemp -d)
trap 'find "$T" -delete' EXIT
# NeXT-ish grayscale: bg,fg, then the 16 ANSI colors (blue = cursor, cyan = range).
THEME=e6e6e6,1a1a1a,000000,8b1a1a,2d5a2d,6b5600,3c3c3c,5a3c5a,b4b4b4,d0d0d0,555555,aa3333,3a7a3a,8a7300,4a4a4a,7a4f7a,c8c8c8,ffffff
agg --idle-time-limit 3 --font-size 15 --theme "$THEME" "docs/demo/$name.cast" "$T/raw.gif"
gifsicle -O3 --lossy=30 --colors 64 "$T/raw.gif" -o "docs/src/img/$name.gif"
# Scrolling text is most of the bits: a wide motion search (merange) finds
# the scrolled lines, which halves the file; one keyframe, at the start.
ffmpeg -loglevel error -y -i "docs/src/img/$name.gif" -movflags +faststart -pix_fmt yuv420p \
	-vf "fps=10,scale=trunc(iw/2)*2:trunc(ih/2)*2" -tune animation -preset veryslow -crf 26 \
	-x264-params keyint=9999:scenecut=0:me=umh:merange=700 "docs/src/img/$name.mp4"
ffmpeg -loglevel error -y -ss "$at" -i "docs/src/img/$name.gif" -frames:v 1 "$T/poster.png"
magick "$T/poster.png" -strip -colors 16 "PNG8:docs/src/img/$name.png"
ls -l "docs/src/img/$name".*
ffprobe -v error -show_entries format=duration -of csv=p=0 "docs/src/img/$name.mp4"
