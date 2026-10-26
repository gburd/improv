#!/usr/bin/env bash
# Linux packages for a release (road-to-1.0 J5), with nfpm:
#   improv_<ver>_amd64.deb, improv-<ver>-1.x86_64.rpm   (the four binaries,
#     man pages, completions) from a release tarball's staging directory;
#   postgresql-<N>-improv_<ver>_amd64.deb, postgresql<N>-improv-<ver>-1.x86_64.rpm
#     (pg_improv for one PG major, PGDG's names and paths) from a
#     `cargo pgrx package` tarball.
#
#   ci/packages.sh bin <ver> <staging-dir> <out-dir>
#   ci/packages.sh pg  <ver> <pg-major> <pgrx-package-tarball> <out-dir>
#
# Needs nfpm on PATH (CI installs a checksum-pinned release).
set -euo pipefail
kind=$1 ver=$2
work=$(mktemp -d)
trap 'find "$work" -delete' EXIT
common='
version: "'"$ver"'"
release: 1
arch: amd64
platform: linux
maintainer: Greg Burd <greg@burd.me>
vendor: Improv
homepage: https://codeberg.org/gregburd/improv
license: MIT OR Apache-2.0
'
case $kind in
bin)
	src=$(cd "$3" && pwd) out=$4
	cat >"$work/nfpm.yaml" <<-EOF
		name: improv
		$common
		description: |-
		  Improv: a multidimensional spreadsheet (Lotus Improv lineage).
		  The improv CLI, improv-tui, improv-server and improv-gui.
		contents:
		  - { src: $src/improv, dst: /usr/bin/improv, file_info: { mode: 0755 } }
		  - { src: $src/improv-tui, dst: /usr/bin/improv-tui, file_info: { mode: 0755 } }
		  - { src: $src/improv-server, dst: /usr/bin/improv-server, file_info: { mode: 0755 } }
		  - { src: $src/improv-gui, dst: /usr/bin/improv-gui, file_info: { mode: 0755 } }
		  - { src: $src/man/man1/*, dst: /usr/share/man/man1/ }
		  - { src: $src/man/man5/*, dst: /usr/share/man/man5/ }
		  - { src: $src/man/man7/*, dst: /usr/share/man/man7/ }
		  # bash-completion loads completions/<command>, without a suffix.
		  - { src: $src/completions/bash/improv.bash, dst: /usr/share/bash-completion/completions/improv }
		  - { src: $src/completions/bash/improv-tui.bash, dst: /usr/share/bash-completion/completions/improv-tui }
		  - { src: $src/completions/bash/improv-server.bash, dst: /usr/share/bash-completion/completions/improv-server }
		  - { src: $src/completions/bash/improv-gui.bash, dst: /usr/share/bash-completion/completions/improv-gui }
		  - { src: $src/completions/zsh/*, dst: /usr/share/zsh/vendor-completions/ }
		  - { src: $src/completions/fish/*, dst: /usr/share/fish/vendor_completions.d/ }
		  - { src: $src/LICENSE-APACHE, dst: /usr/share/doc/improv/LICENSE-APACHE }
		  - { src: $src/LICENSE-MIT, dst: /usr/share/doc/improv/LICENSE-MIT }
		  - { src: $src/README.md, dst: /usr/share/doc/improv/README.md }
	EOF
	;;
pg)
	pg=$3 tarball=$4 out=$5
	mkdir "$work/root"
	tar xzf "$tarball" -C "$work/root"
	test -f "$work/root/usr/lib/postgresql/$pg/lib/pg_improv.so"
	# PGDG's names and paths: postgresql-17-foo in /usr/lib/postgresql/17 and
	# /usr/share/postgresql/17 (apt.postgresql.org), postgresql17-foo in
	# /usr/pgsql-17/{lib,share} (yum.postgresql.org).
	lib=$work/root/usr/lib/postgresql/$pg/lib share=$work/root/usr/share/postgresql/$pg/extension
	test -d "$share"
	for f in deb rpm; do
		if [ $f = deb ]; then
			name=postgresql-$pg-improv dep="postgresql-$pg"
			dlib=/usr/lib/postgresql/$pg/lib dshare=/usr/share/postgresql/$pg/extension
		else
			name=postgresql$pg-improv dep="postgresql$pg-server"
			dlib=/usr/pgsql-$pg/lib dshare=/usr/pgsql-$pg/share/extension
		fi
		cat >"$work/nfpm-$f.yaml" <<-EOF
			name: $name
			$common
			description: |-
			  pg_improv: Improv's multidimensional models over PostgreSQL $pg tables,
			  kept current incrementally, and LANGUAGE improv.
			depends: [ "$dep" ]
			contents:
			  - { src: $lib/, dst: $dlib/, type: tree }
			  - { src: $share/, dst: $dshare/, type: tree }
		EOF
	done
	;;
*) echo "usage: $0 bin|pg ..." >&2; exit 2 ;;
esac
mkdir -p "$out"
if [ "$kind" = bin ]; then
	nfpm package -f "$work/nfpm.yaml" -p deb -t "$out/"
	nfpm package -f "$work/nfpm.yaml" -p rpm -t "$out/"
else
	nfpm package -f "$work/nfpm-deb.yaml" -p deb -t "$out/"
	nfpm package -f "$work/nfpm-rpm.yaml" -p rpm -t "$out/"
fi
ls -l "$out"
