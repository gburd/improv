# Family finance with pg_improv

One family's books for January to June 2026, kept in plain Postgres
tables, with four Improv models on top:

| Model | Answers | Key formulas |
| --- | --- | --- |
| `recon` | Does each account match its bank statement? | `Difference = StatementBalance * BalanceSign - ClearedBalance`, `Reconciled = ABS(Difference) < 0.005`, `UnreconciledCount = SUM(Unreconciled OVER Account)` |
| `budget` | Where did the money go, against the budget? | `Actual = NEG(Outflow)`, `Variance = Actual - Budget`, `OverBudget = Actual > Budget`, `YtdActual = SUM(Actual OVER Month)`, `EmployerMatch = MIN2(K401, Gross * MatchCap) * MatchRate` |
| `networth` | Assets minus liabilities, each month, in dollars | `LedgerValue = Balance * Fx`, `HoldingsValue = SUM(PositionValue OVER Security)`, `NetWorth = Assets - Liabilities` |
| `plan` | Will each savings goal be met? How much more a month if not? | `Projected = Current * Growth + MonthlySaving * Annuity`, `Shortfall = MAX2(Target - ProjectedAtTarget, 0)`, `ExtraMonthly = Shortfall / AnnuityAtTarget` |

The family: two working adults (one also consults on 1099s and sets aside
estimated taxes), two kids. Their accounts: checking, two savings accounts, a
high-yield rainy-day fund, a euro account and euro/yen cash, two credit
cards, a 401(k) with an employer match, traditional and Roth IRAs, a health savings account, a
529 per kid, a brokerage account, a BTC/ETH wallet, a house with a mortgage,
and two cars, one with a loan.

The full walkthrough, with every formula explained and real output, is the
book chapter [A family's finances in Postgres](../../../../docs/src/postgres-finance.md).

## Run it

```sh
crates/pg_improv/tests/finance.sh            # scratch cluster, checks 49 values, ~10 s
KEEP=1 crates/pg_improv/tests/finance.sh     # leave it running (port 55460)
psql -h ~/pgi-finance -p 55460 -U improv family -f crates/pg_improv/examples/family-finance/queries.sql
```

In your own database (pg_improv preloaded, `wal_level = logical`,
`improv.database` set to it):

```sh
psql -d family -f schema.sql -f data.sql -f model.sql -f queries.sql
```

| File | What |
| --- | --- |
| `schema.sql` | Schema `finance`: people, institutions, accounts, currencies and FX rates, the transaction ledger, statements, income and tax set-asides, holdings and prices, valuations, loans and amortization, budget envelopes, savings goals. Keys, checks and comments throughout. |
| `data.sql` | Six months of deterministic data, with planted reconciliation problems. |
| `model.sql` | The four models. Built in manual mode, computed once, then switched to logical sync. |
| `queries.sql` | Fifteen questions a family asks, answered from the result tables, including two `improv.what_if` scenarios. |

## What it catches

June has five planted problems. The reconciliation model finds four and
lists the fifth as pending:

```text
   account   | bank_says | ledger_cleared | difference | difference_usd | pending
-------------+-----------+----------------+------------+----------------+---------
 amex        |   -439.02 |       -1963.14 |    1524.12 |        1524.12 | -412.00   missing autopay leg; hotel hold pending
 visa        |  -1668.40 |       -1754.80 |      86.40 |          86.40 |    0.00   duplicate charge
 checking    |  21402.97 |       21427.97 |     -25.00 |         -25.00 |    0.00   wire fee nobody entered
 eur_account |   9183.10 |        9170.60 |      12.50 |          13.50 |    0.00   FX difference (EUR)
```

`tests/finance.sh` then commits the four fixes as ordinary `INSERT`s and a
`DELETE` on `finance.txn`. Without any `recalc`, the logical-sync worker
brings every June difference to 0 within about 0.2 s. The budget and
net-worth results follow too.

## What is SQL and what is Improv

When this example was written, Improv's formulas had no `IF`, no `POWER`,
no previous-period reference and no dimensionless results (they have all
of these now: `IF`, `^`, `PREV`, running windows); a formula over a
missing cell still has no cell. So the example does these in SQL:

- **Running balances** are SQL: `finance.ledger_month`, a table kept by
  statement triggers on `txn`. It is a table, not a view, because pg_improv
  can sync a view only in manual mode.
- **Compounding factors** `(1 + r/12)^n` are SQL: `finance.goal_horizon`.
- **Choices** are arithmetic: 0/1 columns (`is_essential`, `balance_sign`)
  multiplied in, `MIN2`/`MAX2` for floors and caps, comparisons for flags.
- **Totals** keep one dimension: `NetWorth[Month]`, `Shortfall[Goal]`.
- **Everything else** (differences, flags, variances, year to date, savings
  rate, the employer match, net worth, projections, shortfalls and what-ifs)
  is Improv, in the result tables `improv_<model>.<measure>`.
