-- Family finance example for pg_improv: the questions, answered from the
-- result tables (improv_<model>.<measure>). Every result is an ordinary
-- table: join it, filter it, round it. Values are double precision, so
-- money is rounded to cents for display. Only derived measures have result
-- tables; an input (a bound column) is read from its own table.

\pset footer off
SET search_path = finance, public;

\echo '== 1. Which accounts do not reconcile in June, and by how much?'
SELECT d.account,
       round(b.value::numeric, 2)  AS bank_says,
       c.cleared_balance           AS ledger_cleared,
       round(d.value::numeric, 2)  AS difference,
       round(h.value::numeric, 2)  AS difference_usd,
       round(p.value::numeric, 2)  AS pending
  FROM improv_recon.difference d
  JOIN improv_recon.bankbalance b USING (month, account)
  JOIN improv_recon.differencehome h USING (month, account)
  JOIN improv_recon.pending p USING (month, account)
  JOIN improv_recon.reconciled r USING (month, account)
  JOIN finance.ledger_month c ON (c.month, c.account_id) = (d.month, d.account)
 WHERE d.month = '2026-06' AND NOT r.value
 ORDER BY abs(h.value) DESC;

\echo '== 2. Reconciliation summary by month (accounts off, total off in USD, transfers in balance?)'
SELECT n.month, n.value AS accounts_off, round(o.value::numeric, 2) AS off_by_usd,
       round(t.value::numeric, 2) AS transfers_net, b.value AS transfers_balanced
  FROM improv_recon.unreconciledcount n
  JOIN improv_recon.offbyhome o USING (month)
  JOIN improv_recon.transfersnet t USING (month)
  JOIN improv_recon.transfersbalanced b USING (month)
 ORDER BY n.month;

\echo '== 3. Pending card charges (in the ledger, not yet on the statement)'
SELECT month, account, round(value::numeric, 2) AS pending
  FROM improv_recon.pending WHERE abs(value) >= 0.005 ORDER BY month, account;

\echo '== 4. Where are we over budget in June?'
SELECT v.envelope, b.amount AS budget, round(a.value::numeric, 2) AS actual,
       round(v.value::numeric, 2) AS over_by
  FROM improv_budget.variance v
  JOIN improv_budget.actual a USING (month, envelope)
  JOIN improv_budget.overbudget o USING (month, envelope)
  JOIN finance.budget b ON (b.month, b.envelope_id) = (v.month, v.envelope)
 WHERE v.month = '2026-06' AND o.value
 ORDER BY v.value DESC;

\echo '== 5. Year to date (January-June), by envelope'
SELECT v.envelope, round(b.value::numeric, 2) AS ytd_budget, round(a.value::numeric, 2) AS ytd_actual,
       round(v.value::numeric, 2) AS ytd_variance, o.value AS over
  FROM improv_budget.ytdvariance v
  JOIN improv_budget.ytdactual a USING (envelope)
  JOIN improv_budget.ytdbudget b USING (envelope)
  JOIN improv_budget.ytdoverbudget o USING (envelope)
 ORDER BY v.value DESC;

\echo '== 6. Income, saving and the savings rate, by month'
SELECT g.month, round(g.value::numeric, 2) AS gross, round(n.value::numeric, 2) AS net_pay,
       round(s.value::numeric, 2) AS spending, round(f.value::numeric, 2) AS saved_from_checking,
       round(p.value::numeric, 2) AS saved_by_payroll, round(100 * r.value::numeric, 1) AS savings_rate_pct,
       round(u.value::numeric, 2) AS left_over
  FROM improv_budget.totalgross g
  JOIN improv_budget.totalnetpay n USING (month)
  JOIN improv_budget.spending s USING (month)
  JOIN improv_budget.savedfromchecking f USING (month)
  JOIN improv_budget.payrollsaved p USING (month)
  JOIN improv_budget.savingsrate r USING (month)
  JOIN improv_budget.unallocated u USING (month)
 ORDER BY g.month;

\echo '== 7. Income by source, year to date'
SELECT source, round(value::numeric, 2) AS ytd_gross FROM improv_budget.ytdgross ORDER BY value DESC;

\echo '== 8. Is enough of the consulting income in the tax reserve?'
SELECT g.month, g.source, round(d.value::numeric, 2) AS due, s.amount AS moved,
       round(g.value::numeric, 2) AS short_by
  FROM improv_budget.setasidegap g
  JOIN improv_budget.setasidedue d USING (month, source)
  JOIN finance.tax_setaside s ON (s.month, s.source_id) = (g.month, g.source)
 WHERE g.value >= 0.005 ORDER BY g.month;

\echo '== 9. Net worth trend (USD)'
SELECT w.month, round(c.value::numeric) AS cash, round(i.value::numeric) AS investments,
       round(p.value::numeric) AS property, round(l.value::numeric) AS liabilities,
       round(w.value::numeric) AS net_worth,
       round((w.value - lag(w.value) OVER (ORDER BY w.month))::numeric) AS change,
       round(100 * d.value::numeric, 1) AS debt_to_assets_pct
  FROM improv_networth.networth w
  JOIN improv_networth.cash c USING (month)
  JOIN improv_networth.investments i USING (month)
  JOIN improv_networth.property p USING (month)
  JOIN improv_networth.liabilities l USING (month)
  JOIN improv_networth.debttoassets d USING (month)
 ORDER BY w.month;

\echo '== 10. What it is made of, June 30 (USD)'
SELECT a.account_type, v.account, round(v.value::numeric, 2) AS value_usd
  FROM (SELECT account, value FROM improv_networth.ledgervalue WHERE month = '2026-06'
        UNION ALL SELECT account, value FROM improv_networth.holdingsvalue WHERE month = '2026-06'
        UNION ALL SELECT account_id, value_home FROM finance.valuation WHERE month = '2026-06') v
  JOIN finance.account a ON a.account_id = v.account
 ORDER BY a.account_type, v.value DESC;

\echo '== 11. Are we on track for each goal?'
SELECT s.goal_id AS goal, s.target_date, round(s.current_balance) AS today,
       round(m.value::numeric, 2) AS monthly, round(t.value::numeric) AS target,
       round(p.value::numeric) AS projected, o.value AS on_track, round(f.value::numeric) AS shortfall
  FROM finance.savings_goal s
  JOIN improv_plan.monthlysaving m ON m.goal = s.goal_id
  JOIN improv_plan.target t ON t.goal = s.goal_id
  JOIN improv_plan.projectedattarget p ON p.goal = s.goal_id
  JOIN improv_plan.ontrack o ON o.goal = s.goal_id
  JOIN improv_plan.shortfall f ON f.goal = s.goal_id
 ORDER BY s.target_date, s.goal_id;

\echo '== 12. How much more to save each month to close each shortfall?'
SELECT e.goal, round(m.value::numeric, 2) AS saving_now, round(e.value::numeric, 2) AS add_monthly,
       round(r.value::numeric, 2) AS required_monthly
  FROM improv_plan.extramonthly e
  JOIN improv_plan.monthlysaving m USING (goal)
  JOIN improv_plan.requiredmonthly r USING (goal)
 WHERE e.value >= 0.005
 ORDER BY e.value DESC;

\echo '== 13. Maya''s 529, year by year'
SELECT year, round(p.value::numeric) AS projected, round(100 * pr.value::numeric) AS pct_of_target
  FROM improv_plan.projected p JOIN improv_plan.progress pr USING (goal, year)
 WHERE goal = 'college_maya' ORDER BY year;

\echo '== 14. What if Dana raises the 401(k) deferral from 10% to 15%? (writes nothing)'
SELECT measure, coord ->> 'Goal' AS goal,
       CASE jsonb_typeof(value) WHEN 'number' THEN round(value::numeric, 2)::text ELSE value::text END AS would_be
  FROM improv.what_if('plan', '[{"measure": "DeferralRate", "coord": {"Goal": "retirement"}, "value": 0.15}]')
 WHERE measure IN ('Payroll', 'EmployerMatch', 'MonthlySaving', 'ProjectedAtTarget', 'Shortfall', 'ExtraMonthly', 'OnTrack')
 ORDER BY measure;

\echo '== 15. What if we cut dining out by $100 a month and save it for the trip?'
-- The plan model does not see dining; the cut shows up as $100 more a month
-- into the vacation fund. what_if returns jsonb values: OnTrack is a boolean.
SELECT measure, coord ->> 'Goal' AS goal,
       CASE jsonb_typeof(value) WHEN 'number' THEN round(value::numeric, 2)::text ELSE value::text END AS would_be
  FROM improv.what_if('plan', '[{"measure": "Contribution", "coord": {"Goal": "vacation"}, "value": 350}]')
 WHERE measure IN ('MonthlySaving', 'ProjectedAtTarget', 'Shortfall', 'ExtraMonthly', 'OnTrack')
 ORDER BY measure;
