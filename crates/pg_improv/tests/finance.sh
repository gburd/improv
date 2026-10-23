#!/usr/bin/env bash
# End-to-end test of the family-finance example
# (crates/pg_improv/examples/family-finance): loads the schema, six months of
# books and the four models into a scratch cluster, runs every query, checks
# the key numbers against hand-derived values, then fixes the planted
# reconciliation problems and checks that the logical-sync worker brings the
# result tables up to date on its own.
#
# Needs pg_improv installed into the PG found by PG_BIN (default: Debian's
# PG 17). Uses its own data dir and port; KEEP=1 leaves the cluster running.
#
#   crates/pg_improv/tests/finance.sh
#
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=${PGI_DATA:-$HOME/pgi-finance}
P=${PGI_PORT:-55460}
EX="$(cd "$(dirname "$0")/../examples/family-finance" && pwd)"
T0=$(date +%s)

cleanup() {
	# (-m immediate when a fast stop hangs: the sync worker can miss SIGTERM
	# while it waits for WAL; see docs/reviews/2026-10-23-docs-review.md.)
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast -t 30 >/dev/null 2>&1 ||
		"$PG_BIN/pg_ctl" -D "$D" stop -m immediate >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap '[ -n "${KEEP:-}" ] || cleanup' EXIT
cleanup

"$PG_BIN/initdb" -D "$D" -A trust -U improv >/dev/null
cat >>"$D/postgresql.conf" <<EOF
port = $P
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'family'
improv.poll_ms = 50
wal_level = logical
max_replication_slots = 10
max_wal_senders = 10
EOF
# 3>&- : do not hand the server a pipe the caller is waiting on
"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv family
PSQL=("$PG_BIN/psql" -h "$D" -p "$P" -U improv -d family -X -v ON_ERROR_STOP=1)
q() { "${PSQL[@]}" -qAt -c "$1"; }

for f in schema data model; do
	"${PSQL[@]}" -q -f "$EX/$f.sql" >/dev/null
done
echo "ok: schema.sql, data.sql, model.sql loaded ($(q "SELECT count(*) FROM finance.txn") ledger rows, 4 models)"
OUT=$(mktemp)
"${PSQL[@]}" -f "$EX/queries.sql" >"$OUT"
echo "ok: queries.sql ran ($(grep -c '^== ' "$OUT") questions answered)"

FAILS=0
# check LABEL SQL WANT: SQL must return exactly WANT
check() {
	local got
	got=$(q "$2")
	if [ "$got" = "$3" ]; then
		echo "ok: $1 = $3"
	else
		echo "FAIL: $1: got '$got' want '$3'" >&2
		FAILS=$((FAILS + 1))
	fi
}
# wait_for LABEL SQL WANT: poll until SQL returns WANT (the logical worker
# applies a commit after a poll interval), or fail after 15 s
wait_for() {
	local got="" t0
	t0=$(date +%s%N)
	for _ in $(seq 1 300); do
		got=$(q "$2" || true)
		if [ "$got" = "$3" ]; then
			echo "ok: $1 = $3 (after $(( ($(date +%s%N) - t0) / 1000000 )) ms)"
			return 0
		fi
		sleep 0.05
	done
	echo "FAIL: $1: got '$got' want '$3' after 15 s" >&2
	tail -20 "$D/log" >&2
	FAILS=$((FAILS + 1))
}
cents() { echo "SELECT round(value::numeric, 2) FROM improv_$1"; }

# ── 1. Reconciliation, June, as loaded ────────────────────────────────────
# Difference = statement (in the ledger's sign) - cleared ledger balance.
#  amex      +1524.12  the June autopay's card leg is missing. Autopay pays
#                      the May statement balance, 1524.12 (data.sql: the
#                      2026-05 amex statement row).
#  visa        +86.40  the 2026-06-20 Corner Market charge was entered twice.
#  checking    -25.00  the bank's wire fee is not in the ledger.
#  eur_account +12.50  EUR: the bank credited EUR 1012.50 for a wire entered
#                      as EUR 1000.00; in dollars 12.50 x 1.0800 (June) = 13.50.
D6="recon.difference WHERE month = '2026-06'"
check "Difference[Jun, amex]"         "$(cents "$D6 AND account = 'amex'")" 1524.12
check "Difference[Jun, visa]"         "$(cents "$D6 AND account = 'visa'")" 86.40
check "Difference[Jun, checking]"     "$(cents "$D6 AND account = 'checking'")" -25.00
check "Difference[Jun, eur_account]"  "$(cents "$D6 AND account = 'eur_account'")" 12.50
check "DifferenceHome[Jun, eur_account]" \
	"$(cents "recon.differencehome WHERE month = '2026-06' AND account = 'eur_account'")" 13.50
# Off by 1524.12 + 86.40 + 25.00 + 13.50 = 1649.02 dollars across 4 accounts.
check "UnreconciledCount[Jun]" "SELECT value FROM improv_recon.unreconciledcount WHERE month = '2026-06'" 4
check "OffByHome[Jun]"         "$(cents "recon.offbyhome WHERE month = '2026-06'")" 1649.02
# The missing card leg also unbalances June's transfers: -1524.12.
check "TransfersNet[Jun]"      "$(cents "recon.transfersnet WHERE month = '2026-06'")" -1524.12
# The $412.00 Lakeview Hotel hold is pending: in the ledger, not on the
# statement, and not a reconciliation difference.
check "Pending[Jun, amex]"     "$(cents "recon.pending WHERE month = '2026-06' AND account = 'amex'")" -412.00
# January to May reconcile (the statements were generated from a correct ledger).
check "accounts off, Jan-May"  "SELECT sum(value) FROM improv_recon.unreconciledcount WHERE month < '2026-06'" 0
# Euro and yen cash have no statement, so no Difference cell: 9 accounts x 6 months.
check "Reconciled cells"       "SELECT count(*) FROM improv_recon.reconciled" 54

# ── 2. Budget ─────────────────────────────────────────────────────────────
# June groceries: Corner Market 139.70 + 152.84 + 134.55 + 148.27 (weekly)
# + 86.40 (the 20th) + 86.40 (the duplicate) + Costco 301.95 = 1050.11,
# against a budget of 850.00: Variance 200.11, OverBudget true.
V6="budget.variance WHERE month = '2026-06'"
check "Actual[Jun, groceries]"    "$(cents "budget.actual WHERE month = '2026-06' AND envelope = 'groceries'")" 1050.11
check "Variance[Jun, groceries]"  "$(cents "$V6 AND envelope = 'groceries'")" 200.11
check "OverBudget[Jun, groceries]" \
	"SELECT value FROM improv_budget.overbudget WHERE month = '2026-06' AND envelope = 'groceries'" t
# Housing: mortgage interest + principal 1628.04 + escrow 610.00 = 2238.04
# against 2240.00: 1.96 under.
check "Variance[Jun, housing]"    "$(cents "$V6 AND envelope = 'housing'")" -1.96
# Year to date, fun: 2340.00 spent (cinema 288, skiing 498, flights 1142,
# hotel 412) against 6 x 150.00 = 900.00.
check "YtdVariance[fun]"          "$(cents "budget.ytdvariance WHERE envelope = 'fun'")" 1440.00
# Employer match, Dana, per month: MIN2(401(k) 1300, 6% x gross 13000 = 780)
# x 50% = 390.00. Sam's employers have no match.
check "EmployerMatch[Jun, dana_salary]" \
	"$(cents "budget.employermatch WHERE month = '2026-06' AND source = 'dana_salary'")" 390.00
# Savings rate, June: (from checking 4000 + payroll 401(k) 1300 + health savings 300 +
# match 390) / gross (2 x 6500 + 5400 + consulting 2000 = 20400) = 0.29363.
check "SavingsRate[Jun]" "SELECT round(value::numeric, 5) FROM improv_budget.savingsrate WHERE month = '2026-06'" 0.29363
# May's consulting payment, 4200.00, owed 30% = 1260.00 to the tax reserve;
# only 1000.00 was moved.
check "SetAsideGap[May, sam_design]" \
	"$(cents "budget.setasidegap WHERE month = '2026-05' AND source = 'sam_design'")" 260.00

# ── 3. Net worth ──────────────────────────────────────────────────────────
# Crypto at market, June 30: ETH 2.40 x 3580 = 8592.00; BTC 0.185 plus six
# $100 purchases (100/94250 + 100/88600 + ... + 100/103250, 8 dp)
# = 0.19149207 x 103250 = 19771.56; together 28363.56.
check "HoldingsValue[Jun, crypto_wallet]" \
	"$(cents "networth.holdingsvalue WHERE month = '2026-06' AND account = 'crypto_wallet'")" 28363.56
# Foreign currency at the June rate: EUR 9170.60 x 1.08 = 9904.25,
# EUR 350 cash = 378.00, JPY 48000 x 0.0068 = 326.40.
check "LedgerValue[Jun, eur_account]" \
	"$(cents "networth.ledgervalue WHERE month = '2026-06' AND account = 'eur_account'")" 9904.25
check "LedgerValue[Jun, cash_jpy]" \
	"$(cents "networth.ledgervalue WHERE month = '2026-06' AND account = 'cash_jpy'")" 326.40
# Liabilities, June 30: mortgage 313626.23 + car loan 15837.68 + Visa 1754.80
# + Amex 2375.14 (as the family's ledger has them) = 333593.85.
check "Liabilities[Jun]" "$(cents "networth.liabilities WHERE month = '2026-06'")" 333593.85
# Net worth, June 30 = cash 69277.75 + investments 455096.82 + house and cars
# 594300.00 - liabilities 333593.85 = 785080.72. Cross-checked below against
# the same sum written directly in SQL over the source tables.
check "NetWorth[Jun]" "$(cents "networth.networth WHERE month = '2026-06'")" 785080.72
NW_SQL="SELECT round(
   (SELECT sum(l.balance * l.fx_to_home) FROM finance.ledger_month l WHERE l.month = '2026-06')
 + (SELECT sum(h.units * p.price) FROM finance.holding h JOIN finance.price p USING (month, security_id)
     WHERE h.month = '2026-06')
 + (SELECT sum(value_home) FROM finance.valuation WHERE month = '2026-06'), 2)"
check "NetWorth[Jun], in plain SQL" "$NW_SQL" 785080.72

# ── 4. Planning ───────────────────────────────────────────────────────────
# Vacation (no growth): 50% of joint savings 11411.96 = 5705.98 today, plus
# 12 x 250 by 2027-06-30 = 8705.98, against 9500: Shortfall 794.02, closed
# by 794.02 / 12 = 66.17 more a month.
check "Shortfall[vacation]"    "$(cents "plan.shortfall WHERE goal = 'vacation'")" 794.02
check "ExtraMonthly[vacation]" "$(cents "plan.extramonthly WHERE goal = 'vacation'")" 66.17
# Rainy day: 6 x the average essential budget. Essentials per month:
# housing 2240 + utilities 520 + groceries 850 + transport 150 + childcare
# 780 + healthcare 150 + insurance 251 + car loan 512.30 = 5453.30 (Jan-Mar);
# 5413.30 (Apr-May, utilities 480); 5833.30 (Jun, childcare 1200). Average
# 33019.80 / 6 = 5503.30, so the target is 33019.80.
check "Target[rainy_day]"      "$(cents "plan.target WHERE goal = 'rainy_day'")" 33019.80
# Projected: 24839.17 x (1 + 0.04/12)^12 + 500 x annuity(12) = 31962.39, so
# Shortfall = 33019.80 - 31962.39 = 1057.41.
check "Shortfall[rainy_day]"   "$(cents "plan.shortfall WHERE goal = 'rainy_day'")" 1057.41
# Retirement saving a month: Roth 500 + 401(k) 10% x 156000 / 12 = 1300 +
# match MIN2(10%, 6%) x 50% x 13000 = 390: 2190.00.
check "MonthlySaving[retirement]" "$(cents "plan.monthlysaving WHERE goal = 'retirement'")" 2190.00

# ── 5. What-if (writes nothing) ───────────────────────────────────────────
# Dana defers 15% instead of 10%: +650 a month (the match is capped at 6%, so
# it stays 390), MonthlySaving 2840, and the retirement shortfall goes to 0.
WI="SELECT round(value::numeric, 2) FROM improv.what_if('plan',
  '[{\"measure\": \"DeferralRate\", \"coord\": {\"Goal\": \"retirement\"}, \"value\": 0.15}]')"
check "what_if 15%: MonthlySaving[retirement]" "$WI WHERE measure = 'MonthlySaving'" 2840.00
check "what_if 15%: Shortfall[retirement]"     "$WI WHERE measure = 'Shortfall'" 0.00
check "what_if 15%: EmployerMatch unchanged" \
	"SELECT count(*) FROM improv.what_if('plan', '[{\"measure\": \"DeferralRate\", \"coord\": {\"Goal\": \"retirement\"}, \"value\": 0.15}]') WHERE measure = 'EmployerMatch'" 0
# $100 a month cut from dining, into the trip: 5705.98 + 12 x 350 = 9905.98.
check "what_if +100/mo: ProjectedAtTarget[vacation]" \
	"SELECT round(value::numeric, 2) FROM improv.what_if('plan', '[{\"measure\": \"Contribution\", \"coord\": {\"Goal\": \"vacation\"}, \"value\": 350}]') WHERE measure = 'ProjectedAtTarget'" 9905.98
check "what_if wrote nothing"  "$(cents "plan.shortfall WHERE goal = 'vacation'")" 794.02

# ── 6. Logical sync: fix the books, watch the results follow ──────────────
# The worker starts before createdb, exits, and retries every 5 s; wait
# until it has created the database's one slot (shared by every model since
# 0.11.0) and loaded the models, so the fixes below are applied as changes
# rather than picked up by a first load.
wait_for "replication slot" "SELECT count(*) FROM pg_replication_slots WHERE slot_name = 'improv_db'" 1
wait_for "worker caught up" \
	"SELECT count(*) FROM unnest(ARRAY['recon', 'budget', 'networth', 'plan']) m, improv.status(m) WHERE lag_bytes IS NOT NULL" 4
q "BEGIN;
   -- 1. the missing card leg of the June Amex autopay
   INSERT INTO finance.txn (txn_date, account_id, amount, payee, transfer_id)
   VALUES ('2026-06-25', 'amex', 1524.12, 'Autopay, thank you', 'pay-amex-2026-06');
   -- 2. the duplicate Corner Market charge
   DELETE FROM finance.txn WHERE memo = 'entered twice';
   -- 4. the FX difference, as the bank credited it
   INSERT INTO finance.txn (txn_date, account_id, amount, payee, memo)
   VALUES ('2026-06-05', 'eur_account', 12.50, 'Sparkasse Rhein', 'FX difference on the June wire');
   -- 5. the wire fee
   INSERT INTO finance.txn (txn_date, account_id, amount, payee, envelope_id)
   VALUES ('2026-06-05', 'checking', -25.00, 'Harbor CU: outgoing wire fee', 'bank_fees');
   COMMIT;"
echo "   (committed the four fixes; nothing calls recalc from here on)"
wait_for "Difference[Jun, amex] after fix"        "$(cents "$D6 AND account = 'amex'")" 0.00
wait_for "Difference[Jun, visa] after fix"        "$(cents "$D6 AND account = 'visa'")" 0.00
wait_for "Difference[Jun, checking] after fix"    "$(cents "$D6 AND account = 'checking'")" 0.00
wait_for "Difference[Jun, eur_account] after fix" "$(cents "$D6 AND account = 'eur_account'")" 0.00
wait_for "UnreconciledCount[Jun] after fix" "SELECT value FROM improv_recon.unreconciledcount WHERE month = '2026-06'" 0
wait_for "TransfersBalanced[Jun] after fix" "SELECT value FROM improv_recon.transfersbalanced WHERE month = '2026-06'" t
check "Pending[Jun, amex] still pending"    "$(cents "recon.pending WHERE month = '2026-06' AND account = 'amex'")" -412.00
# Groceries lose the duplicate: 1050.11 - 86.40 = 963.71, variance 113.71.
wait_for "Variance[Jun, groceries] after fix"     "$(cents "$V6 AND envelope = 'groceries'")" 113.71
# Fees: EUR account fee 4.90 x 1.08 = 5.29 + the wire fee 25.00 = 30.29,
# against 6.00: 24.29 over.
wait_for "Variance[Jun, bank_fees] after fix"     "$(cents "$V6 AND envelope = 'bank_fees'")" 24.29
# Net worth: + 1524.12 (Amex owes less) + 86.40 (Visa) - 25.00 + 13.50
# = + 1599.02: 785080.72 -> 786679.74, and still equal to the plain-SQL sum.
wait_for "NetWorth[Jun] after fix" "$(cents "networth.networth WHERE month = '2026-06'")" 786679.74
check "NetWorth[Jun] after fix, in plain SQL" "$NW_SQL" 786679.74
check "worker errors" "SELECT count(*) FROM unnest(ARRAY['recon', 'budget', 'networth', 'plan']) m, improv.status(m) WHERE last_error IS NOT NULL" 0

echo "$(( $(date +%s) - T0 )) s"
if [ "$FAILS" -ne 0 ]; then
	echo "FAIL: $FAILS check(s) failed" >&2
	exit 1
fi
echo "PASS"
