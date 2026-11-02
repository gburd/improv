#!/usr/bin/env bash
# Build improv-cosmic on Solnix (illumos), the way Solnix builds its own
# COSMIC apps (graphics/cosmic-apps.nix there): vendor the locked crates,
# apply Solnix's illumos fixups to the vendored sources, build offline
# without wgpu. Run on the Solnix host, in crates/gui_cosmic:
#
#   ci/solnix-build.sh [cargo build args...]     e.g. --release
#
# Needs: cargo (rustup for x86_64-unknown-illumos), gcc, python3,
# coreutils (sha256sum), and an installed COSMIC session (Solnix's
# graphics fixups and libxkbcommon, wayland, libffi and mesa are taken from
# its cosmic-comp; override with SOLNIX_GRAPHICS and SOLNIX_LIBS).
# Runs: ICED_BACKEND=tiny-skia UMEM_OPTIONS=backend=mmap target/.../improv-cosmic
# (wgpu has no illumos backend; libumem's default sbrk backend fails the
# renderer's first large allocation, as Solnix's app wrappers note).
set -euo pipefail
crate=$(cd "$(dirname "$0")/.." && pwd)
cd "$crate"

die() { echo "solnix-build: $*" >&2; exit 1; }
[ "$(uname -s)" = SunOS ] || die "not illumos"

# Everything comes from the installed COSMIC session: the system's
# cosmic-comp links the client libraries we need, and its derivation names
# the graphics fixups it was built with. So the build matches what runs.
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
# A store path with bin/$2, by name, for tools (several versions may
# coexist; any works).
store() {
	local d
	# $1 is a glob suffix (python3-3.*): it must stay unquoted.
	# shellcheck disable=SC2231
	for d in /nix/store/*-$1; do
		[ -x "$d/bin/$2" ] && echo "$d" && return
	done
	die "no /nix/store/*-$1 with bin/$2"
}

# Tools the fixups call by bare name, from the store when not on PATH.
for t in python3:'python3-3.*' sha256sum:'coreutils-9.*' timeout:'coreutils-9.*'; do
	command -v "${t%%:*}" >/dev/null || PATH="$(store "${t#*:}" "${t%%:*}")/bin:$PATH"
done
export PATH

# Link against the store libraries and record them as RUNPATH.
pc="" rp="" ld=""
for d in $LIBS; do
	pc="$pc:$d/lib/amd64/pkgconfig" rp="$rp:$d/lib/amd64" ld="$ld -L$d/lib/amd64"
done
gcclib=$(dirname "$(gcc -print-file-name=libgcc_s.so.1)")
export PKG_CONFIG_PATH=${pc#:}
mkdir -p target/solnix
cat >target/solnix/cc <<EOF
#!/usr/bin/bash
exec gcc -m64$ld -Wl,-R,${rp#:}:$gcclib "\$@"
EOF
chmod +x target/solnix/cc
export CC=$crate/target/solnix/cc CARGO_TARGET_X86_64_UNKNOWN_ILLUMOS_LINKER=$crate/target/solnix/cc
# rustup's own binaries need libgcc_s too.
export LD_LIBRARY_PATH=$gcclib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
[ -n "${SSL_CERT_FILE:-}" ] || [ ! -f /etc/ssl/certs/ca-certificates.crt ] ||
	export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

# Vendor once per Cargo.lock; the fixups edit the vendored copies (and
# re-stamp their checksums), never the registry.
v=target/solnix/vendor
if ! cmp -s Cargo.lock target/solnix/Cargo.lock.vendored; then
	find "$v" -delete 2>/dev/null || true
	mkdir -p target/solnix/.cargo
	cargo vendor --locked "$v" >target/solnix/.cargo/config.toml
	bash "$G/cosmic-comp-vendor-fixup.sh" "$crate/$v"
	python3 "$G/cosmic-widen-linux.py" "$v" libcosmic 'iced?' 'iced_core?' 'iced_runtime?' 'iced_winit?' 'iced_widget?'
	cp Cargo.lock target/solnix/Cargo.lock.vendored
fi
# cargo vendor prints the directory relative to the crate.
sed -i "s|directory = \".*\"|directory = \"$crate/$v\"|" target/solnix/.cargo/config.toml

cargo --config target/solnix/.cargo/config.toml build --locked --offline "$@"
