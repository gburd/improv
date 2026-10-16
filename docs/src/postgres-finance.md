# A family's finances in Postgres (`pg_improv`)

This chapter walks through a complete example: one family's books for
January to June 2026, kept in ordinary Postgres tables, with four Improv
models on top that reconcile the bank statements, track the budget, add up
net worth, and project the savings goals. The files are in
`crates/pg_improv/examples/family-finance/` and
`crates/pg_improv/tests/finance.sh` runs them end to end and checks the
numbers below.

```sh
crates/pg_improv/tests/finance.sh       # scratch cluster on port 55460, ~10 s
KEEP=1 crates/pg_improv/tests/finance.sh  # leave it running to explore
psql -h ~/pgi-finance -p 55460 -U improv family
```

Or, in a database of your own (with `pg_improv` preloaded, see
[Improv in Postgres](./postgres.md)):

```sh
psql -d family -f schema.sql -f data.sql -f model.sql -f queries.sql
```

## The family

Dana (software engineer, W-2) and Sam (teacher, W-2, plus a design
consulting business paid on 1099s) and their kids Maya (10) and Leo (6).
They have:

- **Bank accounts:** joint checking, a joint savings account that holds
  three sinking funds, a tax-reserve savings account for the consulting
  income, and a high-yield savings rainy-day fund.
- **Foreign currency:** a euro checking account in Germany, plus euro and
  yen cash, converted to dollars at month-end rates.
- **Two credit cards**, paid in full by autopay.
- **Retirement:** Dana's 401(k) with an employer match, Dana's rollover
  traditional IRA, Sam's Roth IRA, and a family health savings account.
- **College:** a 529 plan for each child.
- **Investing:** a joint brokerage account and a crypto wallet (BTC, ETH),
  held as units and valued at each month's price.
- **Property and debt:** a house with a mortgage (interest, principal and
  escrow), a financed SUV with a car loan, and a paid-off sedan.

## The schema at a glance

`schema.sql` creates schema `finance`. The tables fall into five groups:

| Group | Tables | Notes |
| --- | --- | --- |
| Reference | `currency`, `household`, `person`, `period`, `fx_rate`, `institution`, `account`, `envelope` | `account.account_type` is one of checking, savings, credit, loan, mortgage, retirement, 529, health_savings, brokerage, crypto, fx_cash, asset. `account.tracking` says how its value is known: `ledger`, `holdings` or `valuation`. |
| Ledger | `txn`, `ledger_month`, `statement` | `txn` is what the family recorded: date, account, amount, currency, payee, envelope, `transfer_id` pairing the two legs of a transfer, and `status` (cleared or pending). `statement` is what each bank reports. |
| Income and taxes | `income_source`, `invoice`, `income`, `tax_setaside` | W-2 paystubs with withholdings and pre-tax deferrals; consulting invoices and the payments for them, with nothing withheld. |
| Investments, property, debt | `security`, `price`, `holding`, `valuation`, `loan`, `loan_schedule` | Units × price for investment accounts; a market estimate or depreciation for the house and cars; the amortization schedule for the two loans. |
| Plan | `budget`, `plan_settings`, `savings_goal`, `goal_account`, `goal_horizon`, `goal_expense_window` | Monthly budgets per envelope; goals with targets, contributions and return assumptions; precomputed compounding factors. |

Money is `numeric(14,2)` in the account's own currency. Signs follow the
household: a deposit is positive, a charge negative, so a credit card, loan or
mortgage has a negative balance (money owed). Banks report those as a
positive "balance owed", and `account.balance_sign = -1` says how to
compare the two. Every table has a primary key, foreign keys and `CHECK`s
(for example `income.net_deposit` must equal gross minus every withholding
and deferral), and every table has a `COMMENT`.

There are 17 budget envelopes: housing, utilities, groceries, transport,
childcare, kids' activities, healthcare, insurance, subscriptions, dining,
travel and fun, gifts, giving, debt payments, estimated taxes, savings, and
bank fees. Each is `essential`, `discretionary`, `savings`, `tax` or `fees`.

## What Improv does, and what SQL does

Improv's formulas compute over categories: `SUM(Actual OVER Month)` and
`Price * Units`. They have no `IF`, no `POWER`, no "previous month", and a
result must keep at least one dimension. A formula over a missing cell has
no cell either (there is no `COALESCE`). The example handles each of these
in SQL and leaves the rest to the models:

| Need | Where | How |
| --- | --- | --- |
| Running balance per account | SQL: `ledger_month` | A table refreshed by statement-level triggers on `txn` and `fx_rate` (a `MERGE` of the running sum). Improv cannot refer to the previous period, and pg_improv cannot sync from a view (see below). |
| Compound growth `(1 + r/12)^n` and annuity factors | SQL: `goal_horizon` | Filled by `refresh_goal_horizon()` with `power()`. Improv has no `POWER`. |
| "Is this envelope essential?", "is this money owed?" | Columns | 0/1 numbers (`envelope.is_essential`, `account.balance_sign`) multiplied into formulas. Improv has no `IF`, and a boolean cannot be multiplied. |
| A household-wide total | Improv, kept per Month or per Goal | `UnreconciledCount[Month]`, `NetWorth[Month]`. There are no dimensionless results. |
| Loan amortization | SQL: `loan_schedule` | A recursive query, run once when the data is loaded. |
| Everything else | Improv | Differences, flags, variances, year to date, savings rate, the employer match, net worth, projections, shortfalls, what-ifs. |

All four models bind tables, not views. pg_improv can bind a view only in
`manual` mode: logical mode needs `REPLICA IDENTITY` and trigger mode
needs transition tables, and Postgres gives a view neither. So the running
balances are a trigger-maintained table, and that is also what lets the
logical sync below see a new transaction.

## Model 1: reconciliation (`recon`)

**Question:** does each account's ledger match the bank statement, month by
month?

Inputs, per (Month, Account): `LedgerBalance` and `ClearedBalance` (the
running sums from `ledger_month`), `FxRate`, `TransfersHome` (that month's
transfer legs in dollars), `StatementBalance` (from `statement`); per
Account: `BalanceSign`.

```text
BankBalance       = StatementBalance * BalanceSign      -- a card's "$439.02 owed" is -439.02
Difference        = BankBalance - ClearedBalance        -- the bank has not seen pending charges
Reconciled        = ABS(Difference) < 0.005
Pending           = LedgerBalance - ClearedBalance
DifferenceHome    = Difference * FxRate                 -- euros in dollars
Unreconciled      = MIN2(FLOOR(ABS(Difference) * 200), 1)   -- 1 if off by >= half a cent
UnreconciledCount = SUM(Unreconciled OVER Account)      -- per Month
AbsDifferenceHome = ABS(DifferenceHome)
OffByHome         = SUM(AbsDifferenceHome OVER Account)
TransfersNet      = SUM(TransfersHome OVER Account)     -- both legs present: 0
TransfersBalanced = ABS(TransfersNet) < 0.005
```

`Unreconciled` is the `IF` workaround: `FLOOR(|d| × 200)` is at least 1
exactly when `|d| ≥ 0.005`, and `MIN2(…, 1)` caps it, so it is a 0/1 count.

`data.sql` plants five problems in June. The model finds four of them and
lists the fifth as pending:

```text
== 1. Which accounts do not reconcile in June, and by how much?
   account   | bank_says | ledger_cleared | difference | difference_usd | pending
-------------+-----------+----------------+------------+----------------+---------
 amex        |   -439.02 |       -1963.14 |    1524.12 |        1524.12 | -412.00
 visa        |  -1668.40 |       -1754.80 |      86.40 |          86.40 |    0.00
 checking    |  21402.97 |       21427.97 |     -25.00 |         -25.00 |    0.00
 eur_account |   9183.10 |        9170.60 |      12.50 |          13.50 |    0.00

== 2. Reconciliation summary by month (accounts off, total off in USD, transfers in balance?)
  month  | accounts_off | off_by_usd | transfers_net | transfers_balanced
---------+--------------+------------+---------------+--------------------
 2026-05 |            0 |       0.00 |          0.00 | t
 2026-06 |            4 |    1649.02 |      -1524.12 | f
```

Reading it:

- **Amex, +1524.12: a missing transaction.** The June autopay was entered on
  the checking side only. The card's leg is missing, so the ledger still
  owes the May balance (1524.12), and June's transfers do not net to zero
  (`TransfersNet = -1524.12`).
- **Visa, +86.40: a duplicate.** The June 20 Corner Market charge was entered
  twice.
- **Checking, -25.00: a fee nobody entered.** The bank charged an outgoing
  wire fee.
- **Euro account, +12.50 EUR ($13.50): an FX difference.** The family entered
  EUR 1,000 for the June wire; the bank's conversion credited EUR 1,012.50.
- **Amex pending -412.00:** a hotel hold that has not posted. It is in the
  ledger as `pending`, the bank does not show it yet, and `Difference`
  compares cleared balances, so it is not counted as a problem.

### Reconciling a statement

Enter the bank's closing balance (positive for money owed), then look at
`improv_recon.difference` for that month. Fix the ledger and the results
update on their own. `tests/finance.sh` commits these four fixes:

```sql
BEGIN;
INSERT INTO finance.txn (txn_date, account_id, amount, payee, transfer_id)   -- the missing leg
VALUES ('2026-06-25', 'amex', 1524.12, 'Autopay, thank you', 'pay-amex-2026-06');
DELETE FROM finance.txn WHERE memo = 'entered twice';                         -- the duplicate
INSERT INTO finance.txn (txn_date, account_id, amount, payee, memo)           -- the FX difference
VALUES ('2026-06-05', 'eur_account', 12.50, 'Sparkasse Rhein', 'FX difference on the June wire');
INSERT INTO finance.txn (txn_date, account_id, amount, payee, envelope_id)    -- the fee
VALUES ('2026-06-05', 'checking', -25.00, 'Harbor CU: outgoing wire fee', 'bank_fees');
COMMIT;
```

Nothing calls `improv.recalc`. The `txn` triggers refresh `ledger_month`
in the same transaction. The pg_improv worker decodes the commit and applies
the changed rows. Within about 0.2 s every June `Difference` is 0,
`UnreconciledCount` is 0 and `TransfersBalanced` is true. The budget and
net-worth models follow too: groceries lose the duplicate, bank fees gain
the wire fee, and net worth rises by $1,599.02. The pending hotel hold is
still pending.

## Model 2: cash flow and budget (`budget`)

**Question:** where did the money go, against the plan?

`Outflow[Month, Envelope]` is the ledger itself, bound with
`agg => 'sum'`: every `txn` row with an envelope is summed into its
(month, envelope) cell. Rows with no envelope (income, opening balances, the
receiving leg of a transfer) have a NULL dimension and are skipped. `Budget`
comes from the `budget` table, `IsSpending` (0 for the savings envelope)
from `envelope`, and income from the `income` paystubs (again
`agg => 'sum'`, per (Month, Source)).

```text
Actual        = NEG(Outflow)                  -- spending is negative in the ledger
Variance      = Actual - Budget               -- > 0: overspent
OverBudget    = Actual > Budget               -- a boolean result
YtdActual     = SUM(Actual OVER Month)        -- per Envelope, January to date
YtdBudget     = SUM(Budget OVER Month)
YtdVariance   = YtdActual - YtdBudget
Spending      = SUM(SpendingPart OVER Envelope)   -- SpendingPart = Actual * IsSpending
SavedFromChecking = SUM(SavedPart OVER Envelope)  -- SavedPart = Actual * (1 - IsSpending)
EmployerMatch = MIN2(K401, Gross * MatchCap) * MatchRate
PayrollSaved  = SUM(PayrollSavedPart OVER Source) -- 401(k) + health savings deferrals + match
SavingsRate   = (SavedFromChecking + PayrollSaved) / TotalGross
Unallocated   = TotalNetPay - Spending - SavedFromChecking
SetAsideDue   = Gross * SetAsideRate              -- 30% of consulting income
SetAsideGap   = SetAsideDue - SetAside
```

The employer match is a formula on pay: 50% of the 401(k) deferral, counting
deferrals only up to 6% of gross. Dana defers 10% of $6,500 a paycheck, so
the match counts $780 of the $1,300 and adds $390 a month.

```text
== 4. Where are we over budget in June?
 envelope  | budget  | actual  | over_by
-----------+---------+---------+---------
 fun       |  150.00 |  460.00 |  310.00
 childcare | 1200.00 | 1420.00 |  220.00
 groceries |  850.00 | 1050.11 |  200.11
 dining    |  250.00 |  309.05 |   59.05
 gifts     |   50.00 |   89.00 |   39.00
 utilities |  480.00 |  503.69 |   23.69

== 6. Income, saving and the savings rate, by month
  month  |  gross   | net_pay  | spending | saved_from_checking | saved_by_payroll | savings_rate_pct | left_over
---------+----------+----------+----------+---------------------+------------------+------------------+-----------
 2026-05 | 22600.00 | 16152.40 |  7907.48 |             4000.00 |          1990.00 |             26.5 |   4244.92
 2026-06 | 20400.00 | 13952.40 |  9455.65 |             4000.00 |          1990.00 |             29.4 |    496.75

== 8. Is enough of the consulting income in the tax reserve?
  month  |   source   |   due   |  moved  | short_by
---------+------------+---------+---------+----------
 2026-05 | sam_design | 1260.00 | 1000.00 |   260.00
```

June fun is over because of the $412 hotel hold. Groceries include the
duplicate charge until it is fixed. Year to date (query 5), fun is $1,440
over, mostly next summer's flights.

### Setting budgets

A budget is one row per (month, envelope) in `finance.budget`. To change
one, update the row and the variances follow:

```sql
UPDATE finance.budget SET amount = 300 WHERE month = '2026-06' AND envelope_id = 'dining';
```

A new envelope needs an `envelope` row and its `budget` rows. Spending
lands in it as soon as `txn` rows carry its id.

## Model 3: net worth (`networth`)

**Question:** what do we own minus what do we owe, each month, in dollars?

Each account has exactly one of a ledger balance, holdings or a valuation.
A formula over a missing cell has no cell, so the model totals each kind
separately and adds the totals:

```text
LedgerValue   = Balance * Fx                         -- euros and yen at the month-end rate
PositionValue = Units * Price                        -- per (Month, Account, Security)
HoldingsValue = SUM(PositionValue OVER Security)     -- 401(k), IRAs, health savings, 529s, brokerage, crypto
OwnedPart     = LedgerValue * (1 + Sign) / 2         -- Sign is 1 (owned) or -1 (owed)
OwedPart      = NEG(LedgerValue) * (1 - Sign) / 2
Cash          = SUM(OwnedPart OVER Account)
Investments   = SUM(HoldingsValue OVER Account)
Property      = SUM(Valuation OVER Account)          -- house and cars
Liabilities   = SUM(OwedPart OVER Account)           -- cards, car loan, mortgage
Assets        = Cash + Investments + Property
NetWorth      = Assets - Liabilities
DebtToAssets  = Liabilities / Assets
```

Crypto is just another holding: BTC and ETH units times their month-end
price. The mortgage balance falls by each payment's principal (a transfer
from checking). Interest and escrow are spending in the housing envelope.

```text
== 9. Net worth trend (USD)
  month  | cash  | investments | property | liabilities | net_worth | change | debt_to_assets_pct
---------+-------+-------------+----------+-------------+-----------+--------+--------------------
 2026-01 | 50765 |      403326 |   586800 |      337733 |    703158 |        |               32.4
 2026-02 | 52969 |      412645 |   587800 |      336384 |    717030 |  13871 |               31.9
 2026-03 | 59195 |      409315 |   588800 |      335185 |    722124 |   5094 |               31.7
 2026-04 | 61837 |      427766 |   591300 |      333925 |    746978 |  24854 |               30.9
 2026-05 | 68524 |      443452 |   593300 |      334084 |    771191 |  24213 |               30.2
 2026-06 | 69278 |      455097 |   594300 |      333594 |    785081 |  13890 |               29.8
```

(The `change` column is SQL's `lag()` over the result table. Improv has no
previous-period reference, but a result table is an ordinary table.)

## Model 4: planning (`plan`)

**Question:** at today's pace, will each goal be met, and if not, how much
more a month would it take?

There are seven goals in `savings_goal`, each with a target date on a June 30:

| Goal | Funded by | Target |
| --- | --- | --- |
| Retirement (Dana at 62, 2048) | 401(k), traditional IRA, Roth IRA | $2,500,000 |
| College for Maya (2034) and Leo (2037) | each child's 529 | $120,000 each |
| Rainy day (2027) | high-yield savings | 6 months of essential expenses |
| Summer 2027 trip | 50% of joint savings | $9,500 |
| Replace the sedan (2030) | 30% of joint savings | $24,000 |
| New roof (2028) | 20% of joint savings | $8,000 |

`current_balance` is the goal's share of its accounts on June 30. `model.sql`
reads it back from the net-worth model's result tables with an ordinary
`UPDATE … FROM improv_networth.holdingsvalue`.

`goal_horizon` holds, for each goal and each plan anniversary up to its
target date, `Growth = (1 + r/12)^n` and `Annuity = ((1 + r/12)^n − 1) / (r/12)`
(monthly compounding, end-of-month contributions), and `IsTargetYear` (1 on
the target date). SQL computes these because Improv has no `POWER`.

```text
EssentialPart     = Budget * IsEssential
EssentialBudget   = SUM(EssentialPart OVER Envelope)       -- per Month
WeightedEssential = EssentialBudget * Weight               -- Weight = 1/6 per month
MonthlyEssentials = SUM(WeightedEssential OVER Month)      -- the average, per Goal
Target            = TargetAmount + MonthsOfCover * MonthlyEssentials

Payroll           = DeferralRate * Salary / 12             -- 401(k) deferrals
EmployerMatch     = MIN2(DeferralRate, MatchCap) * MatchRate * Salary / 12
MonthlySaving     = Contribution + Payroll + EmployerMatch

Projected         = Current * Growth + MonthlySaving * Annuity   -- per (Goal, Year)
ProjectedAtTarget = SUM(TargetPart OVER Year)              -- TargetPart = Projected * IsTargetYear
AnnuityAtTarget   = SUM(AnnuityPart OVER Year)
Shortfall         = MAX2(Target - ProjectedAtTarget, 0)
OnTrack           = ProjectedAtTarget >= Target
ExtraMonthly      = Shortfall / AnnuityAtTarget            -- the saving that closes the gap
RequiredMonthly   = MonthlySaving + ExtraMonthly
Progress          = Projected / Target
```

The rainy-day target is a formula over the budget: six times the average
monthly essential budget (housing, utilities, groceries, transport,
childcare, healthcare, insurance and the car payment), $5,503.30, so
$33,019.80. Raise childcare's budget and the target moves.
`IsTargetYear` picks the target year's projection out of the series without
an `IF`. `MAX2(…, 0)` floors the shortfall.

```text
== 11. Are we on track for each goal?
      goal       | target_date | today  | monthly | target  | projected | on_track | shortfall
-----------------+-------------+--------+---------+---------+-----------+----------+-----------
 rainy_day       | 2027-06-30  |  24839 |  500.00 |   33020 |     31962 | f        |      1057
 vacation        | 2027-06-30  |   5706 |  250.00 |    9500 |      8706 | f        |       794
 house_projects  | 2028-06-30  |   2282 |   50.00 |    8000 |      3482 | f        |      4518
 car_replacement | 2030-06-30  |   3424 |  100.00 |   24000 |      8224 | f        |     15776
 college_maya    | 2034-06-30  |  35980 |  250.00 |  120000 |     83066 | f        |     36934
 college_leo     | 2037-06-30  |  18766 |  250.00 |  120000 |     76365 | f        |     43635
 retirement      | 2048-06-30  | 306515 | 2190.00 | 2500000 |   2339882 | f        |    160118

== 12. How much more to save each month to close each shortfall?
      goal       | saving_now | add_monthly | required_monthly
-----------------+------------+-------------+------------------
 car_replacement |     100.00 |      328.68 |           428.68
 college_maya    |     250.00 |      313.69 |           563.69
 retirement      |    2190.00 |      293.14 |          2483.14
 college_leo     |     250.00 |      248.63 |           498.63
 house_projects  |      50.00 |      188.23 |           238.23
 rainy_day       |     500.00 |       86.51 |           586.51
 vacation        |     250.00 |       66.17 |           316.17
```

### Reading the planning tables

- `improv_plan.projected` (Goal, Year): the balance at each anniversary. Query
  13 lists Maya's 529 year by year, reaching 69% of its target by 2034.
- `improv_plan.shortfall` and `improv_plan.ontrack` (Goal): the gap at the
  target date.
- `improv_plan.extramonthly` (Goal): how much more to save each month,
  starting now, to close it. The sedan fund needs the most: $328.68 more a
  month.

The vacation line checks by hand: $5,705.98 today + 12 × $250 = $8,705.98,
$794.02 short, closed by $794.02 / 12 = $66.17 more a month.

### Changing goals

Goals are rows. Change a target, contribution or return in `savings_goal`
and the projections follow. Changing a return or a target date changes the
growth factors, so run `SELECT finance.refresh_goal_horizon()` afterwards.

## What-if

`improv.what_if` evaluates the model with some inputs changed, in memory, and
returns only the derived cells that would differ. It writes nothing.

```sql
SELECT measure, coord ->> 'Goal' AS goal, value
  FROM improv.what_if('plan',
       '[{"measure": "DeferralRate", "coord": {"Goal": "retirement"}, "value": 0.15}]')
 WHERE measure IN ('Payroll', 'EmployerMatch', 'MonthlySaving',
                   'ProjectedAtTarget', 'Shortfall', 'ExtraMonthly', 'OnTrack');
```

```text
== 14. What if Dana raises the 401(k) deferral from 10% to 15%? (writes nothing)
      measure      |    goal    |  would_be
-------------------+------------+------------
 ExtraMonthly      | retirement | 0.00
 MonthlySaving     | retirement | 2840.00
 OnTrack           | retirement | true
 Payroll           | retirement | 1950.00
 ProjectedAtTarget | retirement | 2694928.57
 Shortfall         | retirement | 0.00
```

`EmployerMatch` is missing from the answer because it does not change: the
match stops at 6% of pay. The extra $650 a month closes the whole $160,118
retirement gap.

Cutting dining out by $100 a month and putting it toward the trip is a
change to the vacation goal's contribution (the plan model does not see
dining):

```text
== 15. What if we cut dining out by $100 a month and save it for the trip?
      measure      |   goal   | would_be
-------------------+----------+----------
 ExtraMonthly      | vacation | 0.00
 MonthlySaving     | vacation | 350.00
 OnTrack           | vacation | true
 ProjectedAtTarget | vacation | 9905.98
 Shortfall         | vacation | 0.00
```

`what_if` returns `jsonb` values, so `OnTrack` comes back as `true`, not a
number. Cast with care (`value::numeric` fails on a boolean).

## Keeping it current

`model.sql` builds each model in `manual` mode and runs `improv.recalc` once,
so the results are complete when the script returns. Its last statement
switches every model to `logical`. From then on the background worker keeps
them current from committed changes to the bound tables. The test measures
0.1 to 0.2 s from commit to visible result.

## Things to know

- **Results are `double precision`.** A difference of 1524.12 may read
  `1524.1200000000001`. Round for display (`round(value::numeric, 2)`), and
  compare with a tolerance, as `Reconciled` does.
- **Only derived measures get result tables.** Read an input (a bound
  column such as `Budget`) from its own table.
- **Aggregated bindings rebuild.** `Outflow` and the income measures use
  `agg => 'sum'`. In logical mode a change to such a table rebuilds the whole
  `budget` model rather than applying the changed cells. At this size that
  takes about 0.1 s.
- **Fill zeros, don't leave cells out.** `savings_goal` has every column for
  every goal (0 where it does not apply), because `Contribution + Payroll`
  has no cell for a goal with no `Payroll`.

## Files

| File | What |
| --- | --- |
| `schema.sql` | Schema `finance`: tables, keys, checks, comments, the `ledger_month` and `goal_horizon` refreshers. |
| `data.sql` | Six months of books, deterministic, with the five planted problems. |
| `model.sql` | The four models. |
| `queries.sql` | Fifteen questions answered from the result tables, the last two of them what-ifs. |
| `tests/finance.sh` | Runs everything on a scratch cluster and checks 49 values, including the logical-sync fix-up. |
