import sys
import statistics as st, collections
d = collections.defaultdict(list)
for line in open("sys.argv[1] if len(sys.argv) > 1 else "/tmp/mbench.out""):
    t = line.split()
    if len(t) != 4 or t[2] == "FAIL":
        continue
    d[(t[1], t[0])].append((int(t[2]), int(t[3])))
labels = sorted({k[0] for k in d}, key=lambda s: (s.split("-")[-1], s))
print(f"{'step':22}{'0.14 ms':>10}{'1.11 ms':>10}{'delta':>8}{'0.14 MiB':>10}{'1.11 MiB':>10}  (median of 3, A/B alternated)")
for l in labels:
    o = d[(l, "old")]; n = d[(l, "new")]
    if not o or not n:
        continue
    om = st.median([x[0] for x in o]); nm = st.median([x[0] for x in n])
    if l.startswith("dbbytes"):
        print(f"{l:22}{om/2**20:>9.1f}M{nm/2**20:>9.1f}M{(nm/om-1)*100:>7.0f}%")
        continue
    orr = st.median([x[1] for x in o]) / 1024; nr = st.median([x[1] for x in n]) / 1024
    print(f"{l:22}{om:>10.0f}{nm:>10.0f}{(nm/om-1)*100:>7.0f}%{orr:>10.0f}{nr:>10.0f}")
