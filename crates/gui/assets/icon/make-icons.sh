#!/usr/bin/env bash
# Render the Improv icon from improv.svg:
#   improv.png  256 px, the GUI's window icon (crates/gui/src/main.rs)
#   improv.ico  16, 24, 32, 48, 64, 128 and 256 px, embedded in the four
#               Windows binaries (packaging/windows/resource.rs) and used
#               by the installer (packaging/windows/improv.wxs)
# Both are committed; run this after editing improv.svg. rsvg-convert and
# ImageMagick come from `nix shell` when they are not on PATH.
set -euo pipefail

missing=()
for t in rsvg-convert:librsvg magick:imagemagick; do
	command -v "${t%%:*}" >/dev/null || missing+=("nixpkgs#${t#*:}")
done
if ((${#missing[@]})); then
	[[ -z ${MAKE_ICONS_IN_NIX:-} ]] || {
		echo "make-icons: still missing after nix shell: ${missing[*]}" >&2
		exit 1
	}
	MAKE_ICONS_IN_NIX=1 exec nix shell "${missing[@]}" -c bash "$0" "$@"
fi

dir=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -r "$tmp"' EXIT

sizes=(16 24 32 48 64 128 256)
for s in "${sizes[@]}"; do
	rsvg-convert -w "$s" -h "$s" "$dir/improv.svg" -o "$tmp/$s.png"
done
# -strip drops the PNG time and text chunks, so a rerun writes the same
# bytes. The 256 px image goes into the .ico as a PNG, as Windows' own
# icons do (a bitmap that size is 256 KiB); the smaller ones as bitmaps.
magick "$tmp/256.png" -strip "$dir/improv.png"
pngs=()
for s in "${sizes[@]}"; do pngs+=("$tmp/$s.png"); done
magick "${pngs[@]}" -strip -define icon:png-compression-size=256 "$dir/improv.ico"
