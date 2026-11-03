#!/usr/bin/env bash
# Build or test improv-gui (crates/gui) on Solnix (illumos), as
# crates/gui_cosmic/ci/solnix-build.sh builds improv-cosmic: vendor the
# locked crates, apply Solnix's illumos fixups to the vendored sources, then
# glutin's illumos patch (docs/reviews/patches/glutin-illumos.patch, until
# it is in a glutin release), and build offline. Run on the Solnix host, at
# the repository root:
#
#   ci/solnix-gui.sh [build|test] [cargo args...]   e.g. build --release
#
# Needs: cargo (rustup for x86_64-unknown-illumos), gcc, and an installed
# COSMIC session (its cosmic-comp names Solnix's graphics fixups and links
# the libxkbcommon, wayland, libffi and mesa we build against; override with
# SOLNIX_GRAPHICS and SOLNIX_LIBS). python3, coreutils and GNU patch come
# from the Nix store when not on PATH.
# Runs: UMEM_OPTIONS=backend=mmap target/debug/improv-gui [model]
# (libumem's default sbrk backend fails the renderer's first large
# allocation, as Solnix's app wrappers note). eframe draws with glow on
# Mesa's EGL; without a render node it falls back to software.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

die() { echo "solnix-gui: $*" >&2; exit 1; }
[ "$(uname -s)" = SunOS ] || die "not illumos"
cmd=${1:-build}
case $cmd in build | test) shift || true ;; *) die "usage: ci/solnix-gui.sh [build|test] [cargo args...]" ;; esac

# The installed COSMIC session's cosmic-comp: its derivation names the
# graphics fixups, its references the client libraries.
# (illumos has no readlink(1); ls -l shows the symlink's target.)
# shellcheck disable=SC2012 # a store path, no odd characters
comp=$(ls -l /run/current-system/sw/bin/cosmic-comp 2>/dev/null | sed -n 's/.* -> //p')
[ -n "$comp" ] || die "no cosmic-comp in the system profile"
comp=${comp%/bin/cosmic-comp}
ref() { nix-store -q --references "$comp" | grep -- "-$1" | head -1; }
drv=$(nix-store -q --deriver "$comp")
G=${SOLNIX_GRAPHICS:-$(nix --extra-experimental-features nix-command derivation show "$drv" |
	sed -n 's/.*"fixups": "\([^"]*\)".*/\1/p' | head -1)}
[ -f "$G/cosmic-comp-vendor-fixup.sh" ] || die "no Solnix graphics fixups (got '$G'); set SOLNIX_GRAPHICS"
LIBS=${SOLNIX_LIBS:-"$(ref libxkbcommon-) $(ref wayland-) $(ref libffi-) $(ref mesa-)"}
for d in $LIBS; do [ -d "$d/lib/amd64" ] || die "not a library: $d"; done
store() {
	local d
	# $1 is a glob suffix (python3-3.*): it must stay unquoted.
	# shellcheck disable=SC2231
	for d in /nix/store/*-$1; do
		[ -x "$d/bin/$2" ] && echo "$d" && return
	done
	die "no /nix/store/*-$1 with bin/$2"
}
for t in python3:'python3-3.*' sha256sum:'coreutils-9.*' timeout:'coreutils-9.*' patch:'gnupatch-2.*'; do
	command -v "${t%%:*}" >/dev/null || PATH="$(store "${t#*:}" "${t%%:*}")/bin:$PATH"
done
export PATH

# Link against the store libraries and record them as RUNPATH (EGL and the
# Wayland client are dlopen'ed by the executable, which searches it too).
pc="" rp="" ld=""
for d in $LIBS; do
	pc="$pc:$d/lib/amd64/pkgconfig" rp="$rp:$d/lib/amd64" ld="$ld -L$d/lib/amd64"
done
gcclib=$(dirname "$(gcc -print-file-name=libgcc_s.so.1)")
export PKG_CONFIG_PATH=${pc#:}
s=target/solnix
mkdir -p $s
cat >$s/cc <<EOF
#!/usr/bin/bash
exec gcc -m64$ld -Wl,-R,${rp#:}:$gcclib "\$@"
EOF
chmod +x $s/cc
export CC=$root/$s/cc CARGO_TARGET_X86_64_UNKNOWN_ILLUMOS_LINKER=$root/$s/cc
export LD_LIBRARY_PATH=$gcclib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
[ -n "${SSL_CERT_FILE:-}" ] || [ ! -f /etc/ssl/certs/ca-certificates.crt ] ||
	export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

# Vendor once per Cargo.lock and patch; the fixups edit the vendored copies
# (re-stamping their checksums), never the registry.
v=$s/vendor
patchfile=docs/reviews/patches/glutin-illumos.patch
stamp=$(cat Cargo.lock $patchfile | sha256sum | cut -d' ' -f1)
if [ "$(cat $s/vendored 2>/dev/null)" != "$stamp" ]; then
	find "$v" -delete 2>/dev/null || true
	mkdir -p $s/.cargo
	cargo vendor --locked "$v" >$s/.cargo/config.toml
	bash "$G/cosmic-comp-vendor-fixup.sh" "$root/$v"
	# glutin's illumos patch. Its .rs hunks apply as they are; a published
	# crate's Cargo.toml is cargo's normalized form, so the patch's one
	# change there (illumos joins the BSDs' target tables) is made by name.
	awk '/^diff --git /{keep = ($3 !~ /(Cargo\.toml|CHANGELOG\.md)$/)} keep' $patchfile |
		patch -p1 -d "$v" --forward --batch
	edited="glutin_egl_sys/build.rs glutin_egl_sys/src/lib.rs glutin_glx_sys/build.rs glutin_glx_sys/src/lib.rs"
	for c in glutin glutin_glx_sys; do
		f=$v/$c/Cargo.toml
		n=$(grep -c "target_os *= *\"openbsd\"))'" "$f" || true)
		[ "$n" -ge 1 ] || die "$c/Cargo.toml: no BSD target table to widen"
		sed -i "s/target_os *= *\"openbsd\"))'/target_os = \"openbsd\", target_os = \"illumos\"))'/" "$f"
		edited="$edited $c/Cargo.toml"
	done
	for e in $edited; do
		c=${e%%/*} f=${e#*/}
		sum=$(sha256sum "$v/$e" | cut -d' ' -f1)
		grep -q "\"$f\":\"[0-9a-f]\{64\}\"" "$v/$c/.cargo-checksum.json" || die "no checksum entry for $e"
		sed -i "s|\"$f\":\"[0-9a-f]\{64\}\"|\"$f\":\"$sum\"|" "$v/$c/.cargo-checksum.json"
	done
	echo "$stamp" >$s/vendored
fi
# cargo vendor prints the directory relative to the root.
sed -i "s|directory = \".*\"|directory = \"$root/$v\"|" $s/.cargo/config.toml

cargo --config $s/.cargo/config.toml "$cmd" --locked --offline -p improv_gui "$@"
