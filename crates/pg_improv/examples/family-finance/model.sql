-- Family finance example for pg_improv: the models.
--
-- Four models over the tables in schema.sql:
--   recon     statement reconciliation, per (Month, Account)
--   budget    cash flow and budget, per (Month, Envelope) and (Month, Source)
--   networth  assets minus liabilities, per Month, in dollars
--   plan      savings goals projected to their target dates, per (Goal, Year)
--
-- Each model is built in manual mode and computed once with improv.recalc,
-- so its result tables are complete when this script returns. The last
-- step switches every model to logical mode: from then on a background
-- worker keeps the results current as rows in the source tables change.
--
-- Formulas only have cells where every operand has one (there is no
-- COALESCE), and there are no dimensionless results, IF, POWER or
-- previous-period references. The schema works around each: running
-- balances (ledger_month) and compounding factors (goal_horizon) are SQL
-- tables, 0/1 numeric columns stand in for IF, and totals keep one
-- dimension (per Month, per Goal).

CREATE EXTENSION IF NOT EXISTS pg_improv;
SET search_path = finance, public;
SET client_min_messages = warning;   -- set_sync_mode notes each publication it skips

-- ── recon: does the ledger agree with the statements? ─────────────────────

SELECT improv.create_model('recon');
SELECT improv.set_sync_mode('recon', 'manual');

SELECT improv.bind_category('recon', 'Month',   'ledger_month', 'month');
SELECT improv.bind_category('recon', 'Account', 'ledger_month', 'account_id');
SELECT improv.bind_measure ('recon', 'LedgerBalance',  'ledger_month', 'balance');
SELECT improv.bind_measure ('recon', 'ClearedBalance', 'ledger_month', 'cleared_balance');
SELECT improv.bind_measure ('recon', 'FxRate',         'ledger_month', 'fx_to_home');
SELECT improv.bind_measure ('recon', 'TransfersHome',  'ledger_month', 'transfers_home');

SELECT improv.bind_category('recon', 'Month',   'statement', 'month');
SELECT improv.bind_category('recon', 'Account', 'statement', 'account_id');
SELECT improv.bind_measure ('recon', 'StatementBalance', 'statement', 'closing_balance');

SELECT improv.bind_category('recon', 'Account', 'account', 'account_id');
SELECT improv.bind_measure ('recon', 'BalanceSign', 'account', 'balance_sign');

-- The statement in the ledger's sign: a card's "$439.02 owed" is -439.02.
SELECT improv.define('recon', 'BankBalance = StatementBalance * BalanceSign');
-- The bank has not seen pending charges, so compare what has cleared.
SELECT improv.define('recon', 'Difference = BankBalance - ClearedBalance');
SELECT improv.define('recon', 'Reconciled = ABS(Difference) < 0.005');
SELECT improv.define('recon', 'Pending = LedgerBalance - ClearedBalance');
SELECT improv.define('recon', 'DifferenceHome = Difference * FxRate');
-- 1 when the account is off by half a cent or more, else 0 (no IF: FLOOR
-- of |d| x 200 is at least 1 exactly when |d| >= 0.005).
SELECT improv.define('recon', 'Unreconciled = MIN2(FLOOR(ABS(Difference) * 200), 1)');
SELECT improv.define('recon', 'UnreconciledCount = SUM(Unreconciled OVER Account)');
SELECT improv.define('recon', 'AbsDifferenceHome = ABS(DifferenceHome)');
SELECT improv.define('recon', 'OffByHome = SUM(AbsDifferenceHome OVER Account)');
-- Both legs of every transfer are in the ledger when this is zero.
SELECT improv.define('recon', 'TransfersNet = SUM(TransfersHome OVER Account)');
SELECT improv.define('recon', 'TransfersBalanced = ABS(TransfersNet) < 0.005');

SELECT improv.recalc('recon');

-- ── budget: where the money went, against the plan ────────────────────────

SELECT improv.create_model('budget');
SELECT improv.set_sync_mode('budget', 'manual');

-- The ledger, summed per (Month, Envelope). Rows without an envelope
-- (transfers in, income, opening balances) have a NULL dimension and are skipped.
SELECT improv.bind_category('budget', 'Month',    'txn', 'month');
SELECT improv.bind_category('budget', 'Envelope', 'txn', 'envelope_id');
SELECT improv.bind_measure ('budget', 'Outflow', 'txn', 'amount_home', agg => 'sum');

SELECT improv.bind_category('budget', 'Month',    'budget', 'month');
SELECT improv.bind_category('budget', 'Envelope', 'budget', 'envelope_id');
SELECT improv.bind_measure ('budget', 'Budget', 'budget', 'amount');

SELECT improv.bind_category('budget', 'Envelope', 'envelope', 'envelope_id');
SELECT improv.bind_measure ('budget', 'IsSpending', 'envelope', 'is_spending');

SELECT improv.bind_category('budget', 'Month',  'income', 'month');
SELECT improv.bind_category('budget', 'Source', 'income', 'source_id');
SELECT improv.bind_measure ('budget', 'Gross',       'income', 'gross',         agg => 'sum');
SELECT improv.bind_measure ('budget', 'NetPay',      'income', 'net_deposit',   agg => 'sum');
SELECT improv.bind_measure ('budget', 'K401',        'income', 'k401_deferral', agg => 'sum');
SELECT improv.bind_measure ('budget', 'HealthDeferral', 'income', 'health_deferral', agg => 'sum');

SELECT improv.bind_category('budget', 'Source', 'income_source', 'source_id');
SELECT improv.bind_measure ('budget', 'SetAsideRate', 'income_source', 'setaside_rate');
SELECT improv.bind_measure ('budget', 'MatchRate',    'income_source', 'match_rate');
SELECT improv.bind_measure ('budget', 'MatchCap',     'income_source', 'match_cap');

SELECT improv.bind_category('budget', 'Month',  'tax_setaside', 'month');
SELECT improv.bind_category('budget', 'Source', 'tax_setaside', 'source_id');
SELECT improv.bind_measure ('budget', 'SetAside', 'tax_setaside', 'amount');

-- Spending is negative in the ledger; Actual is the positive outflow.
SELECT improv.define('budget', 'Actual = NEG(Outflow)');
SELECT improv.define('budget', 'Variance = Actual - Budget');            -- > 0: overspent
SELECT improv.define('budget', 'OverBudget = Actual > Budget');
SELECT improv.define('budget', 'YtdActual = SUM(Actual OVER Month)');
SELECT improv.define('budget', 'YtdBudget = SUM(Budget OVER Month)');
SELECT improv.define('budget', 'YtdVariance = YtdActual - YtdBudget');
SELECT improv.define('budget', 'YtdOverBudget = YtdActual > YtdBudget');

SELECT improv.define('budget', 'SpendingPart = Actual * IsSpending');
SELECT improv.define('budget', 'Spending = SUM(SpendingPart OVER Envelope)');
SELECT improv.define('budget', 'SavedPart = Actual * (1 - IsSpending)');
SELECT improv.define('budget', 'SavedFromChecking = SUM(SavedPart OVER Envelope)');

-- Income. The employer match is a formula on pay: match_rate x the 401(k)
-- deferral, counting deferrals only up to match_cap x gross.
SELECT improv.define('budget', 'EmployerMatch = MIN2(K401, Gross * MatchCap) * MatchRate');
SELECT improv.define('budget', 'PayrollSavedPart = K401 + HealthDeferral + EmployerMatch');
SELECT improv.define('budget', 'PayrollSaved = SUM(PayrollSavedPart OVER Source)');
SELECT improv.define('budget', 'TotalGross = SUM(Gross OVER Source)');
SELECT improv.define('budget', 'TotalNetPay = SUM(NetPay OVER Source)');
SELECT improv.define('budget', 'YtdGross = SUM(Gross OVER Month)');
SELECT improv.define('budget', 'SavingsRate = (SavedFromChecking + PayrollSaved) / TotalGross');
-- What is left in the bank after spending (taxes included) and saving.
SELECT improv.define('budget', 'Unallocated = TotalNetPay - Spending - SavedFromChecking');
-- 1099 income has nothing withheld: was enough moved to the tax reserve?
SELECT improv.define('budget', 'SetAsideDue = Gross * SetAsideRate');
SELECT improv.define('budget', 'SetAsideGap = SetAsideDue - SetAside');

SELECT improv.recalc('budget');

-- ── networth: what the family owns minus what it owes ─────────────────────

SELECT improv.create_model('networth');
SELECT improv.set_sync_mode('networth', 'manual');

-- Ledger accounts (cash, savings, foreign currency, cards and loans).
SELECT improv.bind_category('networth', 'Month',   'ledger_month', 'month');
SELECT improv.bind_category('networth', 'Account', 'ledger_month', 'account_id');
SELECT improv.bind_measure ('networth', 'Balance', 'ledger_month', 'balance');
SELECT improv.bind_measure ('networth', 'Fx',      'ledger_month', 'fx_to_home');
-- Investment accounts: units x price (retirement, health savings, 529s, brokerage, crypto).
SELECT improv.bind_category('networth', 'Month',    'holding', 'month');
SELECT improv.bind_category('networth', 'Account',  'holding', 'account_id');
SELECT improv.bind_category('networth', 'Security', 'holding', 'security_id');
SELECT improv.bind_measure ('networth', 'Units', 'holding', 'units');
SELECT improv.bind_category('networth', 'Month',    'price', 'month');
SELECT improv.bind_category('networth', 'Security', 'price', 'security_id');
SELECT improv.bind_measure ('networth', 'Price', 'price', 'price');
-- The house and the cars.
SELECT improv.bind_category('networth', 'Month',   'valuation', 'month');
SELECT improv.bind_category('networth', 'Account', 'valuation', 'account_id');
SELECT improv.bind_measure ('networth', 'Valuation', 'valuation', 'value_home');
SELECT improv.bind_category('networth', 'Account', 'account', 'account_id');
SELECT improv.bind_measure ('networth', 'Sign', 'account', 'balance_sign');

SELECT improv.define('networth', 'LedgerValue = Balance * Fx');               -- in dollars
SELECT improv.define('networth', 'PositionValue = Units * Price');
SELECT improv.define('networth', 'HoldingsValue = SUM(PositionValue OVER Security)');
-- Split the ledger accounts by sign: Sign is 1 for what is owned, -1 for
-- what is owed, so (1 + Sign) / 2 and (1 - Sign) / 2 are 0/1 flags.
SELECT improv.define('networth', 'OwnedPart = LedgerValue * (1 + Sign) / 2');
SELECT improv.define('networth', 'OwedPart = NEG(LedgerValue) * (1 - Sign) / 2');
-- Each account has exactly one of a ledger balance, holdings or a
-- valuation, and a formula over a missing cell has no cell, so total each
-- kind per month and add the totals.
SELECT improv.define('networth', 'Cash = SUM(OwnedPart OVER Account)');
SELECT improv.define('networth', 'Investments = SUM(HoldingsValue OVER Account)');
SELECT improv.define('networth', 'Property = SUM(Valuation OVER Account)');
SELECT improv.define('networth', 'Liabilities = SUM(OwedPart OVER Account)');
SELECT improv.define('networth', 'Assets = Cash + Investments + Property');
SELECT improv.define('networth', 'NetWorth = Assets - Liabilities');
SELECT improv.define('networth', 'DebtToAssets = Liabilities / Assets');

SELECT improv.recalc('networth');

-- The plan starts from the June 30 value of each goal's accounts, read
-- back from the net-worth results (a result table is an ordinary table).
UPDATE savings_goal g
   SET current_balance = b.balance
  FROM (SELECT ga.goal_id, round(sum(ga.share * v.value::numeric), 2) AS balance
          FROM goal_account ga
          JOIN (SELECT account, value FROM improv_networth.ledgervalue   WHERE month = '2026-06'
                UNION ALL
                SELECT account, value FROM improv_networth.holdingsvalue WHERE month = '2026-06') v
            ON v.account = ga.account_id
         GROUP BY ga.goal_id) b
 WHERE b.goal_id = g.goal_id;

-- ── plan: are the savings goals on track? ─────────────────────────────────

SELECT improv.create_model('plan');
SELECT improv.set_sync_mode('plan', 'manual');

SELECT improv.bind_category('plan', 'Goal', 'savings_goal', 'goal_id');
SELECT improv.bind_measure ('plan', 'Current',       'savings_goal', 'current_balance');
SELECT improv.bind_measure ('plan', 'Contribution',  'savings_goal', 'monthly_contribution');
SELECT improv.bind_measure ('plan', 'TargetAmount',  'savings_goal', 'target_amount');
SELECT improv.bind_measure ('plan', 'MonthsOfCover', 'savings_goal', 'months_of_cover');
SELECT improv.bind_measure ('plan', 'Salary',        'savings_goal', 'annual_salary');
SELECT improv.bind_measure ('plan', 'DeferralRate',  'savings_goal', 'deferral_rate');
SELECT improv.bind_measure ('plan', 'MatchRate',     'savings_goal', 'match_rate');
SELECT improv.bind_measure ('plan', 'MatchCap',      'savings_goal', 'match_cap');

-- (1 + r/12)^n and the annuity factor, per goal per plan anniversary.
SELECT improv.bind_category('plan', 'Goal', 'goal_horizon', 'goal_id');
SELECT improv.bind_category('plan', 'Year', 'goal_horizon', 'year');
SELECT improv.bind_measure ('plan', 'Growth',       'goal_horizon', 'growth_factor');
SELECT improv.bind_measure ('plan', 'Annuity',      'goal_horizon', 'annuity_factor');
SELECT improv.bind_measure ('plan', 'IsTargetYear', 'goal_horizon', 'is_target_year');

-- The budget, for the rainy-day target (the same table the budget model reads).
SELECT improv.bind_category('plan', 'Month',    'budget', 'month');
SELECT improv.bind_category('plan', 'Envelope', 'budget', 'envelope_id');
SELECT improv.bind_measure ('plan', 'Budget', 'budget', 'amount');
SELECT improv.bind_category('plan', 'Envelope', 'envelope', 'envelope_id');
SELECT improv.bind_measure ('plan', 'IsEssential', 'envelope', 'is_essential');
SELECT improv.bind_category('plan', 'Goal',  'goal_expense_window', 'goal_id');
SELECT improv.bind_category('plan', 'Month', 'goal_expense_window', 'month');
SELECT improv.bind_measure ('plan', 'Weight', 'goal_expense_window', 'weight');

-- Monthly essential expenses: the average essential budget of the window.
SELECT improv.define('plan', 'EssentialPart = Budget * IsEssential');
SELECT improv.define('plan', 'EssentialBudget = SUM(EssentialPart OVER Envelope)');
SELECT improv.define('plan', 'WeightedEssential = EssentialBudget * Weight');
SELECT improv.define('plan', 'MonthlyEssentials = SUM(WeightedEssential OVER Month)');
SELECT improv.define('plan', 'Target = TargetAmount + MonthsOfCover * MonthlyEssentials');

-- The monthly plan: the family's own contribution, plus 401(k) deferrals
-- and the employer match on them (50% of deferrals up to 6% of pay).
SELECT improv.define('plan', 'Payroll = DeferralRate * Salary / 12');
SELECT improv.define('plan', 'EmployerMatch = MIN2(DeferralRate, MatchCap) * MatchRate * Salary / 12');
SELECT improv.define('plan', 'MonthlySaving = Contribution + Payroll + EmployerMatch');

-- Future value: today's balance compounded, plus the contributions'.
SELECT improv.define('plan', 'Projected = Current * Growth + MonthlySaving * Annuity');
SELECT improv.define('plan', 'TargetPart = Projected * IsTargetYear');
SELECT improv.define('plan', 'ProjectedAtTarget = SUM(TargetPart OVER Year)');
SELECT improv.define('plan', 'AnnuityPart = Annuity * IsTargetYear');
SELECT improv.define('plan', 'AnnuityAtTarget = SUM(AnnuityPart OVER Year)');
SELECT improv.define('plan', 'Shortfall = MAX2(Target - ProjectedAtTarget, 0)');
SELECT improv.define('plan', 'OnTrack = ProjectedAtTarget >= Target');
-- The extra end-of-month saving that closes the shortfall by the target date.
SELECT improv.define('plan', 'ExtraMonthly = Shortfall / AnnuityAtTarget');
SELECT improv.define('plan', 'RequiredMonthly = MonthlySaving + ExtraMonthly');
SELECT improv.define('plan', 'Progress = Projected / Target');

SELECT improv.recalc('plan');

-- ── From here on, a background worker keeps every model current ───────────

SELECT improv.set_sync_mode(m, 'logical') FROM unnest(ARRAY['recon', 'budget', 'networth', 'plan']) AS m;
