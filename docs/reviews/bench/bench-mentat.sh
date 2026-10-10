#!/usr/bin/env bash
# A/B storage benchmark: improv CLI built against two Mentat pins.
# usage: bench-mentat.sh <old-bin> <new-bin> <rounds>
# Alternates A/B each round (cancels drift), records wall ms + max RSS KiB.
set -u
OLD=$1 NEW=$2 ROUNDS=${3:-3}
W=/tmp/mbench; mkdir -p $W; cd $W
gen() { # rows cols -> csv with header r,c,v
	python3 - "$1" "$2" >"$W/g$1x$2.csv" <<'PY'
import sys; r,c=int(sys.argv[1]),int(sys.argv[2])
print("r,c,v")
for i in range(r):
    for j in range(c): print(f"r{i},c{j},{(i*31+j*7)%997}")
PY
}
[ -f $W/g100x1000.csv ] || gen 100 1000     # 100k cells
[ -f $W/g200x1000.csv ] || gen 200 1000     # 200k cells
step() { # tag label db cmd... -> "tag label ms rss"
	local tag=$1 label=$2; shift 2
	local s=$(date +%s%N)
	/usr/bin/time -f "%M" -o $W/rss "$@" >/dev/null 2>$W/err || { echo "$tag $label FAIL $(tail -1 $W/err)"; return; }
	echo "$tag $label $(( ($(date +%s%N) - s) / 1000000 )) $(cat $W/rss)"
}
run() { # tag bin size
	local tag=$1 B=$2 sz=$3 db=$W/$1-$3.db
	rm -f $db $db-*
	$B init $db >/dev/null
	$B add-category $db 1 Row >/dev/null; $B add-category $db 2 Col >/dev/null
	step $tag import-$sz  $B import-csv $db $W/g$sz.csv 100 Amount v r:1:Row c:2:Col
	$B add-derived $db 200 Double "Amount * 2" >/dev/null
	step $tag load-list-$sz $B list $db
	step $tag eval-$sz    $B eval $db 200
	step $tag show-$sz    $B show $db 100
	step $tag set-$sz     $B set $db 100 5 --at Row=r1,Col=c1
	step $tag reimport-$sz $B import-csv $db $W/g$sz.csv 101 Amount2 v r:1:Row c:2:Col
	echo "$tag dbbytes-$sz $(stat -c %s $db) 0"
}
for round in $(seq 1 $ROUNDS); do
	for sz in 100x1000 200x1000; do
		if (( round % 2 )); then run old $OLD $sz; run new $NEW $sz; else run new $NEW $sz; run old $OLD $sz; fi
	done
done
