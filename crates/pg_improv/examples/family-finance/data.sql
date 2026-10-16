-- Family finance example for pg_improv: six months of books (January to
-- June 2026) for the Okafor-Lind family. Deterministic: no random(), no now().
--
-- The ledger (txn) is what the family recorded. The statements are what the
-- banks report. June has five planted problems; four make the two disagree:
--
--   1. a missing transaction: the June Amex autopay was entered on the
--      checking side only; the card's leg is missing.
--   2. a duplicate charge: a $86.40 Corner Market charge on the Visa was
--      entered twice.
--   3. a pending card charge: a $412.00 hotel hold on the Amex is in the
--      ledger as 'pending' and not on the statement yet (that one is fine).
--   4. an FX difference: the June wire to the euro account was entered as
--      EUR 1,000.00; the bank's conversion credited EUR 1,012.50.
--   5. a bank fee nobody entered: the $25.00 outgoing-wire fee on checking.
--
-- The reconciliation model finds 1, 2, 4 and 5 and lists 3 as pending;
-- tests/finance.sh fixes them and watches the results follow.

SET search_path = finance, public;

INSERT INTO currency VALUES ('USD', 'US dollar'), ('EUR', 'Euro'), ('JPY', 'Japanese yen');

INSERT INTO household VALUES ('okafor_lind', 'The Okafor-Lind family', 'USD');

INSERT INTO person VALUES
    ('dana', 'okafor_lind', 'Dana Okafor', '1986-04-12', 'adult'),  -- software engineer, W-2
    ('sam',  'okafor_lind', 'Sam Lind',    '1988-09-03', 'adult'),  -- teacher, W-2; design consulting, 1099
    ('maya', 'okafor_lind', 'Maya',        '2016-03-21', 'child'),
    ('leo',  'okafor_lind', 'Leo',         '2019-08-14', 'child');

INSERT INTO period
SELECT to_char(d, 'YYYY-MM'), d::date, (d + interval '1 month - 1 day')::date
  FROM generate_series(date '2026-01-01', date '2026-06-01', interval '1 month') AS d;

-- Month-end rates (USD per unit).
INSERT INTO fx_rate
SELECT p.month, c.currency_id, c.rates[p.n]
  FROM (SELECT month, row_number() OVER (ORDER BY month)::int AS n FROM period) p
 CROSS JOIN (VALUES
    ('USD', ARRAY[1, 1, 1, 1, 1, 1]::numeric[]),
    ('EUR', ARRAY[1.0420, 1.0480, 1.0560, 1.0650, 1.0720, 1.0800]::numeric[]),
    ('JPY', ARRAY[0.00640, 0.00655, 0.00668, 0.00660, 0.00672, 0.00680]::numeric[])
 ) AS c(currency_id, rates);

INSERT INTO institution VALUES
    ('harbor_cu',      'Harbor Credit Union',        'bank'),
    ('summit_online',  'Summit Online Bank',         'bank'),
    ('sparkasse',      'Sparkasse Rhein',            'bank'),
    ('northbank',      'Northbank Visa',             'card_issuer'),
    ('amex',           'American Express',           'card_issuer'),
    ('harbor_mtg',     'Harbor Mortgage',            'lender'),
    ('lakeside_auto',  'Lakeside Auto Credit',       'lender'),
    ('beacon',         'Beacon Retirement Services', 'plan'),
    ('statesaver',     'StateSaver 529 Plan',        'plan'),
    ('healthvest',     'HealthVest Health Savings',             'plan'),
    ('harbor_brokers', 'Harbor Brokerage',           'broker'),
    ('coinvault',      'Coinvault',                  'exchange');

INSERT INTO account (account_id, household_id, institution_id, owner_id, name, account_type,
                     currency_id, tracking, balance_sign) VALUES
    ('checking',      'okafor_lind', 'harbor_cu',      NULL,   'Joint checking',             'checking',   'USD', 'ledger',    1),
    ('joint_savings', 'okafor_lind', 'harbor_cu',      NULL,   'Joint savings (sinking funds)', 'savings', 'USD', 'ledger',    1),
    ('tax_reserve',   'okafor_lind', 'harbor_cu',      'sam',  'Tax reserve savings',        'savings',    'USD', 'ledger',    1),
    ('hysa_rainy',    'okafor_lind', 'summit_online',  NULL,   'High-yield rainy-day fund',  'savings',    'USD', 'ledger',    1),
    ('eur_account',   'okafor_lind', 'sparkasse',      'dana', 'Girokonto (euro account)',   'checking',   'EUR', 'ledger',    1),
    ('cash_eur',      'okafor_lind', NULL,             NULL,   'Euro cash',                  'fx_cash',    'EUR', 'ledger',    1),
    ('cash_jpy',      'okafor_lind', NULL,             NULL,   'Yen cash',                   'fx_cash',    'JPY', 'ledger',    1),
    ('visa',          'okafor_lind', 'northbank',      'dana', 'Northbank Visa',             'credit',     'USD', 'ledger',   -1),
    ('amex',          'okafor_lind', 'amex',           NULL,   'Amex (joint)',               'credit',     'USD', 'ledger',   -1),
    ('mortgage',      'okafor_lind', 'harbor_mtg',     NULL,   'Mortgage, 3.125% fixed',     'mortgage',   'USD', 'ledger',   -1),
    ('car_loan',      'okafor_lind', 'lakeside_auto',  NULL,   'SUV loan, 5.9%',             'loan',       'USD', 'ledger',   -1),
    ('k401_dana',     'okafor_lind', 'beacon',         'dana', 'Dana 401(k)',                'retirement', 'USD', 'holdings',  1),
    ('trad_ira_dana', 'okafor_lind', 'harbor_brokers', 'dana', 'Dana traditional IRA (rollover)', 'retirement', 'USD', 'holdings', 1),
    ('roth_sam',      'okafor_lind', 'harbor_brokers', 'sam',  'Sam Roth IRA',               'retirement', 'USD', 'holdings',  1),
    ('health_savings', 'okafor_lind', 'healthvest',    'dana', 'Family health savings account', 'health_savings', 'USD', 'holdings', 1),
    ('c529_maya',     'okafor_lind', 'statesaver',     'maya', '529 for Maya',               '529',        'USD', 'holdings',  1),
    ('c529_leo',      'okafor_lind', 'statesaver',     'leo',  '529 for Leo',                '529',        'USD', 'holdings',  1),
    ('brokerage',     'okafor_lind', 'harbor_brokers', NULL,   'Joint brokerage',            'brokerage',  'USD', 'holdings',  1),
    ('crypto_wallet', 'okafor_lind', 'coinvault',      'sam',  'Crypto wallet (BTC, ETH)',   'crypto',     'USD', 'holdings',  1),
    ('house',         'okafor_lind', NULL,             NULL,   'House',                      'asset',      'USD', 'valuation', 1),
    ('suv',           'okafor_lind', NULL,             NULL,   '2023 SUV (financed)',        'asset',      'USD', 'valuation', 1),
    ('sedan',         'okafor_lind', NULL,             NULL,   '2016 sedan (paid off)',      'asset',      'USD', 'valuation', 1);

INSERT INTO envelope VALUES
    ('housing',         'Housing (mortgage payment, escrow)', 'essential',     1, 1),
    ('utilities',       'Utilities, phones, internet',        'essential',     1, 1),
    ('groceries',       'Groceries',                          'essential',     1, 1),
    ('transport',       'Fuel and car service',               'essential',     1, 1),
    ('childcare',       'Childcare and day camp',             'essential',     1, 1),
    ('kids_activities', 'Kids'' activities',                  'discretionary', 1, 0),
    ('healthcare',      'Healthcare',                         'essential',     1, 1),
    ('insurance',       'Insurance (auto, life)',             'essential',     1, 1),
    ('subscriptions',   'Subscriptions',                      'discretionary', 1, 0),
    ('dining',          'Dining out',                         'discretionary', 1, 0),
    ('fun',             'Travel and fun',                     'discretionary', 1, 0),
    ('gifts',           'Gifts',                              'discretionary', 1, 0),
    ('giving',          'Giving',                             'discretionary', 1, 0),
    ('debt_payments',   'Debt payments (car loan)',           'essential',     1, 1),
    ('taxes',           'Estimated taxes (1099 income)',      'tax',           1, 0),
    ('savings',         'Savings and investing',              'savings',       0, 0),
    ('bank_fees',       'Bank and FX fees',                   'fees',          1, 0);

-- Monthly budget, January to June. Childcare goes up in June (day camp);
-- estimated taxes are due in January, April and June.
INSERT INTO budget
SELECT p.month, b.envelope_id, b.amounts[p.n]
  FROM (SELECT month, row_number() OVER (ORDER BY month)::int AS n FROM period) p
 CROSS JOIN (VALUES
    ('housing',         ARRAY[2240, 2240, 2240, 2240, 2240, 2240]::numeric[]),
    ('utilities',       ARRAY[520, 520, 520, 480, 480, 480]::numeric[]),
    ('groceries',       ARRAY[850, 850, 850, 850, 850, 850]::numeric[]),
    ('transport',       ARRAY[150, 150, 150, 150, 150, 150]::numeric[]),
    ('childcare',       ARRAY[780, 780, 780, 780, 780, 1200]::numeric[]),
    ('kids_activities', ARRAY[360, 360, 360, 360, 360, 360]::numeric[]),
    ('healthcare',      ARRAY[150, 150, 150, 150, 150, 150]::numeric[]),
    ('insurance',       ARRAY[251, 251, 251, 251, 251, 251]::numeric[]),
    ('subscriptions',   ARRAY[70, 70, 70, 70, 70, 70]::numeric[]),
    ('dining',          ARRAY[250, 250, 250, 250, 250, 250]::numeric[]),
    ('fun',             ARRAY[150, 150, 150, 150, 150, 150]::numeric[]),
    ('gifts',           ARRAY[50, 50, 50, 50, 50, 50]::numeric[]),
    ('giving',          ARRAY[165, 165, 165, 165, 165, 165]::numeric[]),
    ('debt_payments',   ARRAY[512.30, 512.30, 512.30, 512.30, 512.30, 512.30]::numeric[]),
    ('taxes',           ARRAY[1500, 0, 0, 1800, 0, 1800]::numeric[]),
    ('savings',         ARRAY[4000, 4000, 4000, 4000, 4000, 4000]::numeric[]),
    ('bank_fees',       ARRAY[6, 6, 6, 6, 6, 6]::numeric[])
 ) AS b(envelope_id, amounts);

-- ── Income ────────────────────────────────────────────────────────────────

-- Dana's employer matches 50% of 401(k) deferrals up to 6% of pay. Sam's
-- school district has a pension, not a 401(k), so no deferral and no match.
INSERT INTO income_source VALUES
    ('dana_salary', 'dana', 'w2',   'Northwind Systems',        156000, 0,    0.50, 0.06, 'checking'),
    ('sam_salary',  'sam',  'w2',   'Lakeside School District',  64800, 0,    0,    0,    'checking'),
    ('sam_design',  'sam',  '1099', 'Lind Design (consulting)',   NULL, 0.30, 0,    0,    'checking');

-- Dana: paid on the 15th and the last day; 10% to the 401(k), $150 to the
-- health savings account and $280 for the family's medical and dental plan, pre-tax.
INSERT INTO income (source_id, received_on, gross, fed_withholding, state_withholding, fica,
                    k401_deferral, health_deferral, benefits, net_deposit)
SELECT 'dana_salary', d, 6500.00, 820.00, 310.00, 497.25, 650.00, 150.00, 280.00, 3792.75
  FROM period p, LATERAL (VALUES (p.first_day + 14), (p.last_day)) AS v(d);
-- Sam (teacher): paid on the 28th.
INSERT INTO income (source_id, received_on, gross, fed_withholding, state_withholding, fica, net_deposit)
SELECT 'sam_salary', p.first_day + 27, 5400.00, 430.00, 190.00, 413.10, 4366.90 FROM period p;

INSERT INTO invoice VALUES
    ('LD-2025-14', 'sam_design', 'Brightline Bakery',    '2025-12-15', '2026-01-14', 2400.00),
    ('LD-2026-01', 'sam_design', 'Harbor Dental Group',  '2026-02-02', '2026-03-04', 3600.00),
    ('LD-2026-02', 'sam_design', 'Brightline Bakery',    '2026-03-20', '2026-04-19', 1800.00),
    ('LD-2026-03', 'sam_design', 'Northwind Systems',    '2026-04-10', '2026-05-10', 4200.00),
    ('LD-2026-04', 'sam_design', 'Lakeside Food Co-op',  '2026-05-12', '2026-06-11', 2000.00),
    ('LD-2026-05', 'sam_design', 'Harbor Dental Group',  '2026-06-15', '2026-07-15', 2750.00);  -- open
-- Consulting payments: nothing withheld (1099).
INSERT INTO income (source_id, received_on, gross, net_deposit, invoice_id) VALUES
    ('sam_design', '2026-01-20', 2400.00, 2400.00, 'LD-2025-14'),
    ('sam_design', '2026-03-18', 3600.00, 3600.00, 'LD-2026-01'),
    ('sam_design', '2026-04-22', 1800.00, 1800.00, 'LD-2026-02'),
    ('sam_design', '2026-05-14', 4200.00, 4200.00, 'LD-2026-03'),
    ('sam_design', '2026-06-10', 2000.00, 2000.00, 'LD-2026-04');

-- 30% of each consulting payment should go to the tax reserve. In May only
-- $1,000 of the $1,260 due was moved (the budget model flags the gap).
INSERT INTO tax_setaside VALUES
    ('2026-01', 'sam_design',  720.00, 'tax_reserve'),
    ('2026-02', 'sam_design',    0.00, 'tax_reserve'),
    ('2026-03', 'sam_design', 1080.00, 'tax_reserve'),
    ('2026-04', 'sam_design',  540.00, 'tax_reserve'),
    ('2026-05', 'sam_design', 1000.00, 'tax_reserve'),
    ('2026-06', 'sam_design',  600.00, 'tax_reserve');

-- ── Loans ─────────────────────────────────────────────────────────────────

INSERT INTO loan VALUES
    ('mortgage', 318450.00, 0.03125, 1628.04, 610.00, 1,  'checking'),
    ('car_loan',  18400.00, 0.05900,  512.30,   0.00, 10, 'checking');

INSERT INTO loan_schedule (account_id, month, payment, interest, principal, escrow, balance_after)
WITH RECURSIVE s AS (
    SELECT account_id, 1 AS n, principal::numeric AS balance_before FROM loan
    UNION ALL
    SELECT s.account_id, s.n + 1,
           s.balance_before - (l.payment - round(s.balance_before * l.annual_rate / 12, 2))
      FROM s JOIN loan l USING (account_id)
     WHERE s.n < (SELECT count(*) FROM period))
SELECT s.account_id, p.month, l.payment, i.interest, l.payment - i.interest, l.escrow,
       s.balance_before - (l.payment - i.interest)
  FROM s
  JOIN loan l USING (account_id)
  JOIN (SELECT month, row_number() OVER (ORDER BY month)::int AS n FROM period) p ON p.n = s.n
 CROSS JOIN LATERAL (SELECT round(s.balance_before * l.annual_rate / 12, 2) AS interest) i;

-- ── Investments and assets ────────────────────────────────────────────────

INSERT INTO security VALUES
    ('TDF2055',   'Target Date 2055 Fund',    'fund'),
    ('TOTAL_MKT', 'Total US Stock Market Index', 'fund'),
    ('INTL',      'Total International Index', 'fund'),
    ('BOND',      'Total Bond Market Index',  'bond_fund'),
    ('AGE_529',   '529 Age-Based Portfolio',  'fund'),
    ('HEALTH_FUND', 'Health Savings Balanced Fund', 'fund'),
    ('BTC',       'Bitcoin',                  'crypto'),
    ('ETH',       'Ether',                    'crypto');

INSERT INTO price
SELECT p.month, s.security_id, s.prices[p.n]
  FROM (SELECT month, row_number() OVER (ORDER BY month)::int AS n FROM period) p
 CROSS JOIN (VALUES
    ('TDF2055',   ARRAY[142.10, 144.85, 141.30, 146.20, 149.75, 151.40]::numeric[]),
    ('TOTAL_MKT', ARRAY[268.40, 273.15, 265.90, 276.30, 283.10, 287.45]::numeric[]),
    ('INTL',      ARRAY[61.20, 62.05, 63.40, 62.80, 64.15, 65.30]::numeric[]),
    ('BOND',      ARRAY[72.40, 72.10, 72.65, 72.90, 72.55, 73.05]::numeric[]),
    ('AGE_529',   ARRAY[21.40, 21.75, 21.30, 21.95, 22.40, 22.65]::numeric[]),
    ('HEALTH_FUND', ARRAY[38.20, 38.90, 37.85, 39.10, 40.05, 40.60]::numeric[]),
    ('BTC',       ARRAY[94250, 88600, 82150, 91400, 97800, 103250]::numeric[]),
    ('ETH',       ARRAY[3310, 2980, 2640, 3120, 3450, 3580]::numeric[])
 ) AS s(security_id, prices);

-- Units at each month end: the opening position plus each month's purchase
-- (the monthly contribution divided by that month's price). The 401(k) gets
-- $1,300 of deferrals plus the $390 employer match a month; health savings $300 of
-- deferrals; the rest are bought from checking (see the ledger below).
INSERT INTO holding
SELECT p.month, h.account_id, h.security_id,
       h.opening + sum(round(h.monthly / pr.price, h.dp)) OVER (
           PARTITION BY h.account_id, h.security_id ORDER BY p.month)
  FROM (VALUES
    ('k401_dana',     'TDF2055',   1180.0,  1690, 4),
    ('trad_ira_dana', 'INTL',       610.0,     0, 4),
    ('trad_ira_dana', 'BOND',       420.0,     0, 4),
    ('roth_sam',      'TOTAL_MKT',  152.0,   500, 4),
    ('health_savings', 'HEALTH_FUND', 410.0,  300, 4),
    ('c529_maya',     'AGE_529',   1520.0,   250, 4),
    ('c529_leo',      'AGE_529',    760.0,   250, 4),
    ('brokerage',     'TOTAL_MKT',   88.0,  2000, 4),
    ('brokerage',     'INTL',       140.0,     0, 4),
    ('crypto_wallet', 'BTC',       0.185,    100, 8),
    ('crypto_wallet', 'ETH',       2.40,       0, 8)
 ) AS h(account_id, security_id, opening, monthly, dp)
 CROSS JOIN period p
  JOIN price pr ON pr.month = p.month AND pr.security_id = h.security_id;

-- House: a monthly market estimate. Cars: straight-line depreciation.
INSERT INTO valuation
SELECT p.month, v.account_id, v.start_value - v.step * (p.n - 1), v.method
  FROM (SELECT month, row_number() OVER (ORDER BY month)::int AS n FROM period) p
 CROSS JOIN (VALUES ('suv', 32000, 400, 'depreciation'), ('sedan', 9800, 100, 'depreciation'))
       AS v(account_id, start_value, step, method)
UNION ALL
SELECT p.month, 'house', h.value, 'market_estimate'
  FROM (SELECT month, row_number() OVER (ORDER BY month)::int AS n FROM period) p
  JOIN unnest(ARRAY[545000, 546500, 548000, 551000, 553500, 555000]::numeric[])
       WITH ORDINALITY AS h(value, n) ON h.n = p.n;

-- ── The ledger ────────────────────────────────────────────────────────────

-- Opening balances on 1 January (liabilities negative: money owed).
INSERT INTO txn (txn_date, account_id, amount, payee) VALUES
    ('2026-01-01', 'checking',        6850.00, 'Opening balance'),
    ('2026-01-01', 'joint_savings',   9000.00, 'Opening balance'),
    ('2026-01-01', 'tax_reserve',     2150.00, 'Opening balance'),
    ('2026-01-01', 'hysa_rainy',     21400.00, 'Opening balance'),
    ('2026-01-01', 'eur_account',     8200.00, 'Opening balance'),
    ('2026-01-01', 'cash_eur',         350.00, 'Opening balance'),
    ('2026-01-01', 'cash_jpy',       48000.00, 'Opening balance'),
    ('2026-01-01', 'visa',           -2184.33, 'Opening balance'),
    ('2026-01-01', 'amex',           -1036.58, 'Opening balance'),
    ('2026-01-01', 'mortgage',     -318450.00, 'Opening balance'),
    ('2026-01-01', 'car_loan',      -18400.00, 'Opening balance');

-- Paychecks and consulting payments land in checking.
INSERT INTO txn (txn_date, account_id, amount, payee, memo)
SELECT i.received_on, s.deposit_account, i.net_deposit, s.payer,
       CASE s.kind WHEN 'w2' THEN 'payroll' ELSE 'invoice ' || i.invoice_id END
  FROM income i JOIN income_source s USING (source_id)
 ORDER BY i.received_on, i.source_id;

-- Recurring spending: one row per payee, the day of the month, and the six
-- monthly amounts (NULL: no charge that month).
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id)
SELECT p.first_day + (r.day - 1), r.account_id, -a.amount, r.payee, r.envelope_id
  FROM (VALUES
    ('checking', 1,  'Bright Futures After-School', 'childcare',       ARRAY[780, 780, 780, 780, 780, NULL]::numeric[]),
    ('checking', 3,  'City Water',                  'utilities',       ARRAY[64.20, 61.80, 66.45, 70.10, 78.35, 91.60]::numeric[]),
    ('checking', 8,  'Metro Electric',              'utilities',       ARRAY[142.18, 131.40, 118.75, 96.30, 104.62, 158.90]::numeric[]),
    ('checking', 8,  'Northern Gas',                'utilities',       ARRAY[128.40, 117.25, 92.10, 61.80, 44.35, 38.20]::numeric[]),
    ('checking', 12, 'Lakeside Swim Club',          'kids_activities', ARRAY[185, 185, 185, 185, 185, 185]::numeric[]),
    ('checking', 15, 'Camp Pinewood',               'childcare',       ARRAY[NULL, NULL, NULL, NULL, NULL, 1420]::numeric[]),
    ('checking', 18, 'Safe Harbor Auto Insurance',  'insurance',       ARRAY[186, 186, 186, 186, 186, 186]::numeric[]),
    ('checking', 18, 'Evergreen Life',              'insurance',       ARRAY[64.50, 64.50, 64.50, 64.50, 64.50, 64.50]::numeric[]),
    ('checking', 20, 'Lakeside Food Bank',          'giving',          ARRAY[150, 150, 150, 150, 150, 150]::numeric[]),
    ('checking', 22, 'Lakeside Dental',             'healthcare',      ARRAY[NULL, 240, NULL, NULL, 180, NULL]::numeric[]),
    ('visa', 3,  'Corner Market',            'groceries',       ARRAY[142.36, 128.90, 151.22, 137.48, 146.05, 139.70]::numeric[]),
    ('visa', 10, 'Corner Market',            'groceries',       ARRAY[118.44, 133.75, 121.60, 144.12, 129.38, 152.84]::numeric[]),
    ('visa', 17, 'Corner Market',            'groceries',       ARRAY[156.02, 119.67, 138.45, 127.93, 141.26, 134.55]::numeric[]),
    ('visa', 24, 'Corner Market',            'groceries',       ARRAY[131.18, 147.30, 126.84, 150.61, 135.92, 148.27]::numeric[]),
    ('visa', 6,  'Costco',                   'groceries',       ARRAY[286.40, 241.15, 312.80, 268.35, 294.60, 301.95]::numeric[]),
    ('visa', 7,  'Shell',                    'transport',       ARRAY[58.40, 61.25, 54.80, 63.10, 66.45, 69.20]::numeric[]),
    ('visa', 21, 'Shell',                    'transport',       ARRAY[52.75, 49.90, 60.35, 57.40, 62.80, 64.15]::numeric[]),
    ('visa', 14, 'Cascade Wireless',         'utilities',       ARRAY[135, 135, 135, 135, 135, 135]::numeric[]),
    ('visa', 14, 'Lakeside Fiber',           'utilities',       ARRAY[79.99, 79.99, 79.99, 79.99, 79.99, 79.99]::numeric[]),
    ('visa', 5,  'Allegro Piano Studio',     'kids_activities', ARRAY[160, 160, 160, 160, 160, 160]::numeric[]),
    ('visa', 16, 'Rite Pharmacy',            'healthcare',      ARRAY[24.50, 38.20, 12.75, 41.60, 18.90, 27.35]::numeric[]),
    ('visa', 9,  'Lakeside Pediatrics',      'healthcare',      ARRAY[40, NULL, 40, NULL, NULL, 80]::numeric[]),
    ('amex', 1,  'Lakeside Courier',         'subscriptions',   ARRAY[17, 17, 17, 17, 17, 17]::numeric[]),
    ('amex', 11, 'StreamFlix',               'subscriptions',   ARRAY[22.99, 22.99, 22.99, 22.99, 22.99, 22.99]::numeric[]),
    ('amex', 11, 'Tunely',                   'subscriptions',   ARRAY[16.99, 16.99, 16.99, 16.99, 16.99, 16.99]::numeric[]),
    ('amex', 11, 'CloudBox',                 'subscriptions',   ARRAY[9.99, 9.99, 9.99, 9.99, 9.99, 9.99]::numeric[]),
    ('amex', 2,  'Bean There Coffee',        'dining',          ARRAY[38.25, 41.10, 36.90, 44.35, 39.80, 47.60]::numeric[]),
    ('amex', 9,  'Luigi''s Trattoria',       'dining',          ARRAY[92.40, 78.15, 104.30, 88.60, 112.75, 131.20]::numeric[]),
    ('amex', 16, 'Taqueria Sol',             'dining',          ARRAY[46.80, 52.35, 39.90, 61.25, 48.70, 57.95]::numeric[]),
    ('amex', 27, 'Pizza Palace',             'dining',          ARRAY[54.20, 48.75, 62.10, 51.40, 58.90, 72.30]::numeric[]),
    ('amex', 13, 'Lakeside Cinema',          'fun',             ARRAY[58, 42, 36, 64, 40, 48]::numeric[]),
    ('amex', 15, 'Public Radio',             'giving',          ARRAY[15, 15, 15, 15, 15, 15]::numeric[]),
    ('eur_account', 28, 'Sparkasse Rhein: account fee', 'bank_fees',     ARRAY[4.90, 4.90, 4.90, 4.90, 4.90, 4.90]::numeric[])
 ) AS r(account_id, day, payee, envelope_id, amounts)
 CROSS JOIN LATERAL unnest(r.amounts) WITH ORDINALITY AS a(amount, n)
  JOIN (SELECT month, first_day, row_number() OVER (ORDER BY month)::int AS n FROM period) p ON p.n = a.n
 WHERE a.amount IS NOT NULL;

-- One-off spending.
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id) VALUES
    ('2026-01-17', 'visa',  -35.00, 'Toy Barn',             'gifts'),         -- a friend's birthday party
    ('2026-01-24', 'amex', -312.00, 'Pine Ridge Ski Area',  'fun'),
    ('2026-02-07', 'amex', -186.00, 'Pine Ridge Ski Area',  'fun'),
    ('2026-02-13', 'visa',  -64.00, 'Bloom Florist',        'gifts'),
    ('2026-02-18', 'visa',  -89.95, 'Lakeside Auto Service', 'transport'),
    ('2026-03-14', 'amex', -134.50, 'Sports Locker',        'kids_activities'),
    ('2026-03-19', 'visa', -120.00, 'Toy Barn',             'gifts'),         -- Maya's birthday
    ('2026-04-02', 'amex', -225.00, 'Lakeside Youth Soccer', 'kids_activities'),
    ('2026-05-09', 'visa',  -72.00, 'Bloom Florist',        'gifts'),
    ('2026-05-18', 'visa', -412.60, 'Lakeside Auto Service', 'transport'),    -- brakes
    ('2026-05-28', 'amex', -1142.00, 'SkyWay Airlines',     'fun'),           -- next summer's flights
    ('2026-06-19', 'visa',  -89.00, 'Harbor Hardware',      'gifts'),
    ('2026-06-20', 'visa',  -86.40, 'Corner Market',        'groceries');

-- Planted problem 2: the June 20 Corner Market charge, entered a second time.
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id, memo) VALUES
    ('2026-06-20', 'visa', -86.40, 'Corner Market', 'groceries', 'entered twice');
-- Planted problem 3: a hotel hold the card has not posted yet.
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id, status) VALUES
    ('2026-06-29', 'amex', -412.00, 'Lakeview Hotel', 'fun', 'pending');

-- Interest the banks paid.
INSERT INTO txn (txn_date, account_id, amount, payee)
SELECT p.last_day, i.account_id, a.amount, 'Interest'
  FROM (VALUES ('hysa_rainy',    ARRAY[71.42, 66.10, 74.36, 73.51, 77.30, 76.48]::numeric[]),
               ('joint_savings', ARRAY[1.85, 1.72, 1.98, 2.04, 2.16, 2.21]::numeric[])) AS i(account_id, amounts)
 CROSS JOIN LATERAL unnest(i.amounts) WITH ORDINALITY AS a(amount, n)
  JOIN (SELECT month, last_day, row_number() OVER (ORDER BY month)::int AS n FROM period) p ON p.n = a.n;

-- Monthly savings, on the 2nd (transfers between ledger accounts) and the
-- 5th (contributions that leave the ledger for an investment account).
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id, transfer_id)
SELECT p.first_day + 1, leg.account_id, leg.amount, leg.payee, leg.envelope_id,
       'save-' || s.to_account || '-' || p.month
  FROM period p
 CROSS JOIN (VALUES ('hysa_rainy', 500.00), ('joint_savings', 400.00)) AS s(to_account, amount)
 CROSS JOIN LATERAL (VALUES ('checking', -s.amount, 'Transfer to ' || s.to_account, 'savings'),
                            (s.to_account, s.amount, 'Transfer from checking', NULL)) AS leg(account_id, amount, payee, envelope_id);
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id)
SELECT p.first_day + 4, 'checking', -c.amount, c.payee, 'savings'
  FROM period p
 CROSS JOIN (VALUES ('Harbor Brokerage: Roth IRA (Sam)', 500.00),
                    ('StateSaver 529: Maya',             250.00),
                    ('StateSaver 529: Leo',              250.00),
                    ('Harbor Brokerage: joint account', 2000.00),
                    ('Coinvault: BTC purchase',          100.00)) AS c(payee, amount);

-- Loan payments: interest and escrow are spending; principal is a transfer
-- that lowers the balance owed.
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id, transfer_id)
SELECT p.first_day + (l.pay_day - 1), leg.account_id, leg.amount, leg.payee,
       leg.envelope_id, leg.transfer_id
  FROM loan_schedule s
  JOIN loan l USING (account_id)
  JOIN period p USING (month)
 CROSS JOIN LATERAL (SELECT CASE l.account_id WHEN 'mortgage' THEN 'housing' ELSE 'debt_payments' END AS env,
                            i.name AS lender, 'loan-' || l.account_id || '-' || s.month AS tid
                       FROM account a JOIN institution i USING (institution_id)
                      WHERE a.account_id = l.account_id) AS x
 CROSS JOIN LATERAL (VALUES
    (l.from_account, -s.interest,  x.lender || ': interest',  x.env, NULL),
    (l.from_account, -s.escrow,    x.lender || ': escrow',    x.env, NULL),
    (l.from_account, -s.principal, x.lender || ': principal', x.env, x.tid),
    (l.account_id,    s.principal, 'Payment: principal',      NULL,  x.tid)
 ) AS leg(account_id, amount, payee, envelope_id, transfer_id)
 WHERE leg.amount <> 0;

-- The consulting tax set-asides (a transfer on the day the payment landed)
-- and the quarterly estimated payments out of the reserve.
INSERT INTO txn (txn_date, account_id, amount, payee, transfer_id)
SELECT i.received_on, leg.account_id, leg.amount, leg.payee, 'setaside-' || t.month
  FROM tax_setaside t
  JOIN income i ON i.source_id = t.source_id AND i.month = t.month
 CROSS JOIN LATERAL (VALUES ('checking', -t.amount, 'Tax set-aside'),
                            (t.to_account, t.amount, 'Tax set-aside')) AS leg(account_id, amount, payee)
 WHERE t.amount > 0;
INSERT INTO txn (txn_date, account_id, amount, payee, envelope_id, memo) VALUES
    ('2026-01-15', 'tax_reserve', -1500.00, 'IRS (EFTPS)', 'taxes', '2025 Q4 estimate'),
    ('2026-04-15', 'tax_reserve', -1800.00, 'IRS (EFTPS)', 'taxes', '2026 Q1 estimate'),
    ('2026-06-15', 'tax_reserve', -1800.00, 'IRS (EFTPS)', 'taxes', '2026 Q2 estimate');

-- Planted problems 4 and 5: the June wire to the euro account. Checking sent
-- $1,100.00; the family entered EUR 1,000.00 on arrival, valued at the
-- $1,100.00 it cost so the transfer nets to zero. The bank credited
-- EUR 1,012.50, and charged a $25.00 wire fee that nobody entered.
INSERT INTO txn (txn_date, account_id, amount, amount_home, payee, transfer_id) VALUES
    ('2026-06-05', 'checking',    -1100.00, NULL,    'Wire to Sparkasse Rhein', 'fx-2026-06'),
    ('2026-06-05', 'eur_account',  1000.00, 1100.00, 'Wire from Harbor CU',     'fx-2026-06');

-- Card autopay on the 25th: last month's statement balance (January: the
-- opening balance). Must run after every charge is in.
INSERT INTO txn (txn_date, account_id, amount, payee, transfer_id)
SELECT p.first_day + 24, leg.account_id, leg.amount, leg.payee, 'pay-' || o.card || '-' || p.month
  FROM (SELECT account_id AS card, to_char(first_day + interval '1 month', 'YYYY-MM') AS month,
               -sum(amount) AS owed
          FROM txn JOIN period USING (month)
         WHERE account_id IN ('visa', 'amex') AND transfer_id IS NULL AND payee <> 'Opening balance'
         GROUP BY 1, 2
        UNION ALL
        SELECT account_id, '2026-01', -amount FROM txn
         WHERE account_id IN ('visa', 'amex') AND payee = 'Opening balance') AS o
  JOIN period p USING (month)
 CROSS JOIN LATERAL (VALUES ('checking', -o.owed, 'Autopay ' || o.card),
                            (o.card,      o.owed, 'Autopay, thank you')) AS leg(account_id, amount, payee)
 -- Planted problem 1: the card's side of the June Amex autopay was never entered.
 WHERE NOT (leg.account_id = 'amex' AND p.month = '2026-06');

-- ── Statements, as the institutions report them ───────────────────────────
-- Closing balance on the last day of each month, in the account's currency;
-- cards and loans report the (positive) balance owed.
INSERT INTO statement (month, account_id, closing_date, closing_balance) VALUES
    ('2026-01', 'amex', '2026-01-31', 683.62),
    ('2026-02', 'amex', '2026-02-28', 530.32),
    ('2026-03', 'amex', '2026-03-31', 495.67),
    ('2026-04', 'amex', '2026-04-30', 616.57),
    ('2026-05', 'amex', '2026-05-31', 1524.12),
    ('2026-06', 'amex', '2026-06-30', 439.02),
    ('2026-01', 'car_loan', '2026-01-31', 17978.17),
    ('2026-02', 'car_loan', '2026-02-28', 17554.26),
    ('2026-03', 'car_loan', '2026-03-31', 17128.27),
    ('2026-04', 'car_loan', '2026-04-30', 16700.18),
    ('2026-05', 'car_loan', '2026-05-31', 16269.99),
    ('2026-06', 'car_loan', '2026-06-30', 15837.68),
    ('2026-01', 'checking', '2026-01-31', 8810.87),
    ('2026-02', 'checking', '2026-02-28', 9993.32),
    ('2026-03', 'checking', '2026-03-31', 14093.20),
    ('2026-04', 'checking', '2026-04-30', 16952.09),
    ('2026-05', 'checking', '2026-05-31', 21599.18),
    ('2026-06', 'checking', '2026-06-30', 21402.97),
    ('2026-01', 'eur_account', '2026-01-31', 8195.10),
    ('2026-02', 'eur_account', '2026-02-28', 8190.20),
    ('2026-03', 'eur_account', '2026-03-31', 8185.30),
    ('2026-04', 'eur_account', '2026-04-30', 8180.40),
    ('2026-05', 'eur_account', '2026-05-31', 8175.50),
    ('2026-06', 'eur_account', '2026-06-30', 9183.10),
    ('2026-01', 'hysa_rainy', '2026-01-31', 21971.42),
    ('2026-02', 'hysa_rainy', '2026-02-28', 22537.52),
    ('2026-03', 'hysa_rainy', '2026-03-31', 23111.88),
    ('2026-04', 'hysa_rainy', '2026-04-30', 23685.39),
    ('2026-05', 'hysa_rainy', '2026-05-31', 24262.69),
    ('2026-06', 'hysa_rainy', '2026-06-30', 24839.17),
    ('2026-01', 'joint_savings', '2026-01-31', 9401.85),
    ('2026-02', 'joint_savings', '2026-02-28', 9803.57),
    ('2026-03', 'joint_savings', '2026-03-31', 10205.55),
    ('2026-04', 'joint_savings', '2026-04-30', 10607.59),
    ('2026-05', 'joint_savings', '2026-05-31', 11009.75),
    ('2026-06', 'joint_savings', '2026-06-30', 11411.96),
    ('2026-01', 'mortgage', '2026-01-31', 317651.26),
    ('2026-02', 'mortgage', '2026-02-28', 316850.44),
    ('2026-03', 'mortgage', '2026-03-31', 316047.53),
    ('2026-04', 'mortgage', '2026-04-30', 315242.53),
    ('2026-05', 'mortgage', '2026-05-31', 314435.43),
    ('2026-06', 'mortgage', '2026-06-30', 313626.23),
    ('2026-01', 'tax_reserve', '2026-01-31', 1370.00),
    ('2026-02', 'tax_reserve', '2026-02-28', 1370.00),
    ('2026-03', 'tax_reserve', '2026-03-31', 2450.00),
    ('2026-04', 'tax_reserve', '2026-04-30', 1190.00),
    ('2026-05', 'tax_reserve', '2026-05-31', 2190.00),
    ('2026-06', 'tax_reserve', '2026-06-30', 990.00),
    ('2026-01', 'visa', '2026-01-31', 1420.04),
    ('2026-02', 'visa', '2026-02-28', 1449.06),
    ('2026-03', 'visa', '2026-03-31', 1513.80),
    ('2026-04', 'visa', '2026-04-30', 1365.58),
    ('2026-05', 'visa', '2026-05-31', 1854.95),
    ('2026-06', 'visa', '2026-06-30', 1668.40);

-- ── Plan ──────────────────────────────────────────────────────────────────

INSERT INTO plan_settings VALUES (true, '2026-06-30');

-- Payroll terms (salary, deferral, match) are plan assumptions; they start
-- from Dana's paystub and income_source, copied below.
INSERT INTO savings_goal (goal_id, name, kind, person_id, target_amount, months_of_cover, target_date,
                          monthly_contribution, annual_return) VALUES
    ('retirement',      'Retire when Dana is 62',     'retirement',   'dana', 2500000, 0, '2048-06-30', 500, 0.06),
    ('college_maya',    'College for Maya (age 18)',  'college',      'maya',  120000, 0, '2034-06-30', 250, 0.05),
    ('college_leo',     'College for Leo (age 18)',   'college',      'leo',   120000, 0, '2037-06-30', 250, 0.05),
    ('rainy_day',       'Rainy day: 6 months of essentials', 'rainy_day', NULL,     0, 6, '2027-06-30', 500, 0.04),
    ('vacation',        'Summer 2027 trip to Germany', 'sinking_fund', NULL,  9500, 0, '2027-06-30', 250, 0),
    ('car_replacement', 'Replace the sedan',          'sinking_fund', NULL,  24000, 0, '2030-06-30', 100, 0),
    ('house_projects',  'New roof',                   'sinking_fund', NULL,   8000, 0, '2028-06-30',  50, 0);
UPDATE savings_goal g
   SET annual_salary = s.annual_salary,
       deferral_rate = (SELECT sum(k401_deferral) / sum(gross) FROM income i WHERE i.source_id = s.source_id),
       match_rate = s.match_rate, match_cap = s.match_cap
  FROM income_source s
 WHERE g.goal_id = 'retirement' AND s.source_id = 'dana_salary';

INSERT INTO goal_account VALUES
    ('retirement',      'k401_dana',     1),
    ('retirement',      'trad_ira_dana', 1),
    ('retirement',      'roth_sam',      1),
    ('college_maya',    'c529_maya',     1),
    ('college_leo',     'c529_leo',      1),
    ('rainy_day',       'hysa_rainy',    1),
    ('vacation',        'joint_savings', 0.5),
    ('car_replacement', 'joint_savings', 0.3),
    ('house_projects',  'joint_savings', 0.2);

-- "Monthly essential expenses" = the average of the six months' essential budgets.
INSERT INTO goal_expense_window
SELECT g.goal_id, p.month, 1.0 / 6 FROM savings_goal g CROSS JOIN period p;

SELECT refresh_goal_horizon();
-- savings_goal.current_balance is set in model.sql, from the net-worth results.
