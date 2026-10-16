-- Family finance example for pg_improv: the schema.
--
-- Plain Postgres. Every table here is ordinary data that the family (or a
-- bank import) writes; model.sql binds some of them to pg_improv models.
--
-- Conventions
--   * Money is numeric(14,2) in the account's own currency, unless the
--     column name ends in _home (US dollars, the household's home currency).
--   * Signs are the household's view of each account: + raises the balance,
--     - lowers it. A credit card, loan or mortgage balance is therefore
--     negative (money owed). Banks report those as a positive "balance
--     owed"; account.balance_sign (-1) says how to compare the two.
--   * month is text 'YYYY-MM'. pg_improv dimension values are text, and a
--     plain column (unlike a generated one) replicates through logical
--     decoding on Postgres 17, so BEFORE triggers fill it from the date.
--   * Tables pg_improv binds have a PRIMARY KEY on their dimension columns,
--     or (txn, income) are bound with agg => 'sum'.

CREATE SCHEMA finance;
SET search_path = finance, public;

-- ── Reference data ────────────────────────────────────────────────────────

CREATE TABLE currency (
    currency_id char(3) PRIMARY KEY CHECK (currency_id ~ '^[A-Z]{3}$'),
    name        text NOT NULL
);
COMMENT ON TABLE currency IS 'ISO 4217 currencies the household holds.';

CREATE TABLE household (
    household_id  text PRIMARY KEY,
    name          text NOT NULL,
    home_currency char(3) NOT NULL REFERENCES currency
);
COMMENT ON TABLE household IS 'The family. Everything is reported in home_currency.';

CREATE TABLE person (
    person_id    text PRIMARY KEY,
    household_id text NOT NULL REFERENCES household,
    name         text NOT NULL,
    born         date NOT NULL,
    role         text NOT NULL CHECK (role IN ('adult', 'child'))
);
COMMENT ON TABLE person IS 'Members of the household.';

CREATE TABLE period (
    month     text PRIMARY KEY CHECK (month ~ '^[0-9]{4}-[0-9]{2}$'),
    first_day date NOT NULL UNIQUE,
    last_day  date NOT NULL,
    CHECK (to_char(first_day, 'YYYY-MM') = month),
    CHECK (last_day = (first_day + interval '1 month - 1 day')::date)
);
COMMENT ON TABLE period IS 'The months the books cover. Every dated row belongs to one.';

CREATE TABLE fx_rate (
    month        text NOT NULL REFERENCES period,
    currency_id  char(3) NOT NULL REFERENCES currency,
    rate_to_home numeric(14, 8) NOT NULL CHECK (rate_to_home > 0),
    PRIMARY KEY (month, currency_id)
);
COMMENT ON TABLE fx_rate IS
    'Month-end rate: 1 unit of currency_id = rate_to_home home units. The home currency has rate 1.';

CREATE TABLE institution (
    institution_id text PRIMARY KEY,
    name           text NOT NULL,
    kind           text NOT NULL CHECK (kind IN ('bank', 'card_issuer', 'broker', 'plan', 'lender', 'exchange'))
);
COMMENT ON TABLE institution IS 'Banks, card issuers, brokers, plan administrators and lenders.';

CREATE TABLE account (
    account_id     text PRIMARY KEY CHECK (account_id ~ '^[a-z][a-z0-9_]*$'),
    household_id   text NOT NULL REFERENCES household,
    institution_id text REFERENCES institution,          -- NULL: cash in a drawer, the house
    owner_id       text REFERENCES person,               -- NULL: joint
    name           text NOT NULL,
    account_type   text NOT NULL CHECK (account_type IN
        ('checking', 'savings', 'credit', 'loan', 'mortgage', 'retirement', '529', 'health_savings',
         'brokerage', 'crypto', 'fx_cash', 'asset')),
    currency_id    char(3) NOT NULL REFERENCES currency,
    tracking       text NOT NULL CHECK (tracking IN ('ledger', 'holdings', 'valuation')),
    balance_sign   smallint NOT NULL CHECK (balance_sign IN (-1, 1)),
    CHECK ((account_type IN ('credit', 'loan', 'mortgage')) = (balance_sign = -1)),
    CHECK (balance_sign = 1 OR tracking = 'ledger')
);
COMMENT ON TABLE account IS
    'Every place money or value sits. tracking says how its value is known: '
    'ledger (sum of txn rows), holdings (units x price) or valuation (an estimate).';
COMMENT ON COLUMN account.balance_sign IS
    '-1 for liabilities: the bank reports owed money as positive, the ledger as negative.';

CREATE TABLE envelope (
    envelope_id  text PRIMARY KEY CHECK (envelope_id ~ '^[a-z][a-z0-9_]*$'),
    name         text NOT NULL,
    kind         text NOT NULL CHECK (kind IN ('essential', 'discretionary', 'savings', 'tax', 'fees')),
    -- 0/1 numbers, not booleans: Improv formulas multiply by them, and a
    -- boolean cannot be multiplied.
    is_spending  numeric(1) NOT NULL CHECK (is_spending = CASE WHEN kind = 'savings' THEN 0 ELSE 1 END),
    is_essential numeric(1) NOT NULL CHECK (is_essential = CASE WHEN kind = 'essential' THEN 1 ELSE 0 END)
);
COMMENT ON TABLE envelope IS
    'Budget envelopes. Essential envelopes size the rainy-day fund; savings is not spending.';

-- ── The ledger ────────────────────────────────────────────────────────────

CREATE TABLE txn (
    txn_id      bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    txn_date    date NOT NULL,
    month       text NOT NULL REFERENCES period,     -- filled from txn_date
    account_id  text NOT NULL REFERENCES account,
    amount      numeric(14, 2) NOT NULL CHECK (amount <> 0),
    currency_id char(3) NOT NULL REFERENCES currency, -- filled from the account
    amount_home numeric(14, 2) NOT NULL,              -- filled: amount x the month's rate, unless given
    payee       text NOT NULL,
    memo        text,
    envelope_id text REFERENCES envelope,
    transfer_id text,
    status      text NOT NULL DEFAULT 'cleared' CHECK (status IN ('cleared', 'pending'))
);
COMMENT ON TABLE txn IS
    'The household ledger: what the family recorded, one row per account movement. '
    'Spending carries an envelope. The two legs of a transfer share a transfer_id; the paying leg also '
    'carries an envelope when the transfer is budgeted (savings, loan principal), card payments carry none. '
    'Income deposits and opening balances carry neither.';
COMMENT ON COLUMN txn.status IS 'pending: authorized, not yet posted by the bank.';
CREATE INDEX ON txn (account_id, month);
CREATE INDEX ON txn (transfer_id) WHERE transfer_id IS NOT NULL;

CREATE FUNCTION txn_fill() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    acct_currency char(3);
    rate numeric;
BEGIN
    NEW.month := to_char(NEW.txn_date, 'YYYY-MM');
    SELECT currency_id INTO acct_currency FROM finance.account WHERE account_id = NEW.account_id;
    IF NEW.currency_id IS NULL THEN
        NEW.currency_id := acct_currency;
    ELSIF NEW.currency_id <> acct_currency THEN
        RAISE EXCEPTION 'txn on % is in %, but the account is in %',
            NEW.account_id, NEW.currency_id, acct_currency;
    END IF;
    SELECT rate_to_home INTO rate FROM finance.fx_rate
     WHERE month = NEW.month AND currency_id = NEW.currency_id;
    IF rate IS NULL THEN
        RAISE EXCEPTION 'no % rate for %', NEW.currency_id, NEW.month;
    END IF;
    -- A home-currency amount is its own home value. A foreign amount is
    -- valued at the month's rate unless the row says what it cost (the
    -- receiving leg of a currency transfer carries the dollars paid, so the
    -- transfer nets to zero).
    IF rate = 1 OR NEW.amount_home IS NULL THEN
        NEW.amount_home := round(NEW.amount * rate, 2);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER txn_fill BEFORE INSERT OR UPDATE ON txn FOR EACH ROW EXECUTE FUNCTION txn_fill();

-- Fill a month column from the date column named in TG_ARGV[0].
CREATE FUNCTION fill_month() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW := jsonb_populate_record(NEW, jsonb_build_object(
        'month', to_char((to_jsonb(NEW) ->> TG_ARGV[0])::date, 'YYYY-MM')));
    RETURN NEW;
END $$;

CREATE TABLE ledger_month (
    month           text NOT NULL REFERENCES period,
    account_id      text NOT NULL REFERENCES account,
    balance         numeric(14, 2) NOT NULL,
    cleared_balance numeric(14, 2) NOT NULL,
    fx_to_home      numeric(14, 8) NOT NULL,
    transfers_home  numeric(14, 2) NOT NULL,
    PRIMARY KEY (month, account_id)
);
COMMENT ON TABLE ledger_month IS
    'Running sum of the ledger: each ledger account''s balance at each month end, in its own currency, '
    'all rows and cleared rows only, with the month-end rate, plus that month''s transfer legs in home '
    'currency (summed over all accounts they must net to zero). Kept current by triggers on txn and fx_rate. '
    'It exists because an Improv formula cannot refer to the previous period.';

CREATE FUNCTION refresh_ledger_month() RETURNS void LANGUAGE sql AS $$
    WITH want AS (
        SELECT p.month, a.account_id,
               COALESCE(sum(t.amount), 0) AS balance,
               COALESCE(sum(t.amount) FILTER (WHERE t.status = 'cleared'), 0) AS cleared_balance,
               r.rate_to_home,
               COALESCE(sum(t.amount_home) FILTER (WHERE t.month = p.month AND t.transfer_id IS NOT NULL), 0)
                   AS transfers_home
          FROM finance.period p
          CROSS JOIN finance.account a
          JOIN finance.fx_rate r ON r.month = p.month AND r.currency_id = a.currency_id
          LEFT JOIN finance.txn t ON t.account_id = a.account_id AND t.month <= p.month
         WHERE a.tracking = 'ledger'
         GROUP BY p.month, a.account_id, r.rate_to_home)
    MERGE INTO finance.ledger_month l
    USING want w ON l.month = w.month AND l.account_id = w.account_id
    WHEN MATCHED AND (l.balance, l.cleared_balance, l.fx_to_home, l.transfers_home)
                     IS DISTINCT FROM (w.balance, w.cleared_balance, w.rate_to_home, w.transfers_home) THEN
        UPDATE SET balance = w.balance, cleared_balance = w.cleared_balance,
                   fx_to_home = w.rate_to_home, transfers_home = w.transfers_home
    WHEN NOT MATCHED THEN
        INSERT VALUES (w.month, w.account_id, w.balance, w.cleared_balance, w.rate_to_home, w.transfers_home)
    WHEN NOT MATCHED BY SOURCE THEN DELETE;
$$;
CREATE FUNCTION ledger_changed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM finance.refresh_ledger_month();
    RETURN NULL;
END $$;
CREATE TRIGGER txn_ledger AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON txn
    FOR EACH STATEMENT EXECUTE FUNCTION ledger_changed();
CREATE TRIGGER fx_ledger AFTER INSERT OR UPDATE OR DELETE ON fx_rate
    FOR EACH STATEMENT EXECUTE FUNCTION ledger_changed();

CREATE TABLE statement (
    month           text NOT NULL REFERENCES period,
    account_id      text NOT NULL REFERENCES account,
    closing_date    date NOT NULL,
    closing_balance numeric(14, 2) NOT NULL,
    PRIMARY KEY (month, account_id)
);
COMMENT ON TABLE statement IS
    'Monthly statement closing balance as the institution reports it, in the account currency. '
    'For credit cards, loans and the mortgage this is the positive balance owed.';

-- ── Income and taxes ──────────────────────────────────────────────────────

CREATE TABLE income_source (
    source_id       text PRIMARY KEY CHECK (source_id ~ '^[a-z][a-z0-9_]*$'),
    person_id       text NOT NULL REFERENCES person,
    kind            text NOT NULL CHECK (kind IN ('w2', '1099')),
    payer           text NOT NULL,
    annual_salary   numeric(14, 2) CHECK (annual_salary > 0),
    setaside_rate   numeric(5, 4) NOT NULL DEFAULT 0 CHECK (setaside_rate BETWEEN 0 AND 1),
    match_rate      numeric(5, 4) NOT NULL DEFAULT 0 CHECK (match_rate BETWEEN 0 AND 1),
    match_cap       numeric(5, 4) NOT NULL DEFAULT 0 CHECK (match_cap BETWEEN 0 AND 1),
    deposit_account text NOT NULL REFERENCES account,
    CHECK ((kind = 'w2') = (annual_salary IS NOT NULL)),
    CHECK (kind = '1099' OR setaside_rate = 0),
    CHECK (kind = 'w2' OR match_rate = 0)
);
COMMENT ON TABLE income_source IS
    'Jobs and the consulting business. setaside_rate: share of 1099 income moved to the tax reserve, '
    'since nothing is withheld from it. match_rate/match_cap: the employer puts match_rate x the 401(k) '
    'deferral into the plan, on deferrals up to match_cap x gross pay.';

CREATE TABLE invoice (
    invoice_id text PRIMARY KEY,
    source_id  text NOT NULL REFERENCES income_source,
    client     text NOT NULL,
    issued_on  date NOT NULL,
    due_on     date NOT NULL CHECK (due_on >= issued_on),
    amount     numeric(14, 2) NOT NULL CHECK (amount > 0)
);
COMMENT ON TABLE invoice IS 'Consulting invoices. Payments arrive as income rows.';

CREATE TABLE income (
    income_id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source_id         text NOT NULL REFERENCES income_source,
    received_on       date NOT NULL,
    month             text NOT NULL REFERENCES period,  -- filled from received_on
    gross             numeric(14, 2) NOT NULL CHECK (gross > 0),
    fed_withholding   numeric(14, 2) NOT NULL DEFAULT 0 CHECK (fed_withholding >= 0),
    state_withholding numeric(14, 2) NOT NULL DEFAULT 0 CHECK (state_withholding >= 0),
    fica              numeric(14, 2) NOT NULL DEFAULT 0 CHECK (fica >= 0),
    k401_deferral     numeric(14, 2) NOT NULL DEFAULT 0 CHECK (k401_deferral >= 0),
    health_deferral      numeric(14, 2) NOT NULL DEFAULT 0 CHECK (health_deferral >= 0),
    benefits          numeric(14, 2) NOT NULL DEFAULT 0 CHECK (benefits >= 0),
    net_deposit       numeric(14, 2) NOT NULL,
    invoice_id        text REFERENCES invoice,
    CHECK (net_deposit = gross - fed_withholding - state_withholding - fica
                         - k401_deferral - health_deferral - benefits)
);
COMMENT ON TABLE income IS
    'One row per paystub (W-2, with withholdings and pre-tax deferrals) or consulting payment '
    '(1099, nothing withheld). net_deposit is what lands in the deposit account.';
CREATE TRIGGER income_month BEFORE INSERT OR UPDATE ON income
    FOR EACH ROW EXECUTE FUNCTION fill_month('received_on');

CREATE TABLE tax_setaside (
    month      text NOT NULL REFERENCES period,
    source_id  text NOT NULL REFERENCES income_source,
    amount     numeric(14, 2) NOT NULL CHECK (amount >= 0),
    to_account text NOT NULL REFERENCES account,
    PRIMARY KEY (month, source_id)
);
COMMENT ON TABLE tax_setaside IS
    'What was actually moved to the tax reserve for each 1099 source each month '
    '(the ledger has the matching transfer). Quarterly estimated payments are txn rows in envelope taxes.';

-- ── Investments, assets and debt ──────────────────────────────────────────

CREATE TABLE security (
    security_id text PRIMARY KEY CHECK (security_id ~ '^[A-Z][A-Z0-9_]*$'),
    name        text NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('fund', 'stock', 'bond_fund', 'cash', 'crypto'))
);
COMMENT ON TABLE security IS 'Funds, coins and cash positions held in investment accounts. Priced in home currency.';

CREATE TABLE price (
    month       text NOT NULL REFERENCES period,
    security_id text NOT NULL REFERENCES security,
    price       numeric(18, 6) NOT NULL CHECK (price > 0),
    PRIMARY KEY (month, security_id)
);
COMMENT ON TABLE price IS 'Month-end price per unit, in home currency.';

CREATE TABLE holding (
    month       text NOT NULL REFERENCES period,
    account_id  text NOT NULL REFERENCES account,
    security_id text NOT NULL REFERENCES security,
    units       numeric(20, 8) NOT NULL CHECK (units >= 0),
    PRIMARY KEY (month, account_id, security_id)
);
COMMENT ON TABLE holding IS 'Units held at month end, from each broker or plan statement (or the wallet).';

CREATE TABLE valuation (
    month      text NOT NULL REFERENCES period,
    account_id text NOT NULL REFERENCES account,
    value_home numeric(14, 2) NOT NULL CHECK (value_home >= 0),
    method     text NOT NULL CHECK (method IN ('market_estimate', 'depreciation', 'appraisal')),
    PRIMARY KEY (month, account_id)
);
COMMENT ON TABLE valuation IS 'Month-end value of a house or car: a market estimate or a depreciation schedule.';

CREATE TABLE loan (
    account_id  text PRIMARY KEY REFERENCES account,
    principal   numeric(14, 2) NOT NULL CHECK (principal > 0),   -- balance before the first payment below
    annual_rate numeric(7, 5) NOT NULL CHECK (annual_rate >= 0),
    payment     numeric(14, 2) NOT NULL CHECK (payment > 0),     -- principal + interest
    escrow      numeric(14, 2) NOT NULL DEFAULT 0 CHECK (escrow >= 0),
    pay_day     int NOT NULL CHECK (pay_day BETWEEN 1 AND 28),
    from_account text NOT NULL REFERENCES account
);
COMMENT ON TABLE loan IS 'Mortgage and car loan terms, as of the first month of the books.';

CREATE TABLE loan_schedule (
    account_id    text NOT NULL REFERENCES loan,
    month         text NOT NULL REFERENCES period,
    payment       numeric(14, 2) NOT NULL,
    interest      numeric(14, 2) NOT NULL CHECK (interest >= 0),
    principal     numeric(14, 2) NOT NULL CHECK (principal >= 0),
    escrow        numeric(14, 2) NOT NULL,
    balance_after numeric(14, 2) NOT NULL CHECK (balance_after >= 0),
    PRIMARY KEY (account_id, month),
    CHECK (payment = interest + principal)
);
COMMENT ON TABLE loan_schedule IS
    'Amortization: how each payment splits into interest (an expense) and principal (a transfer that '
    'lowers the balance owed). Escrow (taxes, insurance) is paid with it.';

-- ── Budgets and goals ─────────────────────────────────────────────────────

CREATE TABLE budget (
    month       text NOT NULL REFERENCES period,
    envelope_id text NOT NULL REFERENCES envelope,
    amount      numeric(14, 2) NOT NULL CHECK (amount >= 0),
    PRIMARY KEY (month, envelope_id)
);
COMMENT ON TABLE budget IS 'Planned outflow per envelope per month, in home currency.';

CREATE TABLE plan_settings (
    only_row bool PRIMARY KEY DEFAULT true CHECK (only_row),
    as_of    date NOT NULL
);
COMMENT ON TABLE plan_settings IS 'The date the plan starts from (the last statement date).';

CREATE TABLE savings_goal (
    goal_id              text PRIMARY KEY CHECK (goal_id ~ '^[a-z][a-z0-9_]*$'),
    name                 text NOT NULL,
    kind                 text NOT NULL CHECK (kind IN ('retirement', 'college', 'rainy_day', 'sinking_fund')),
    person_id            text REFERENCES person,
    target_amount        numeric(14, 2) NOT NULL DEFAULT 0 CHECK (target_amount >= 0),
    months_of_cover      numeric(5, 2) NOT NULL DEFAULT 0 CHECK (months_of_cover >= 0),
    target_date          date NOT NULL CHECK (extract(month FROM target_date) = 6 AND extract(day FROM target_date) = 30),
    monthly_contribution numeric(14, 2) NOT NULL DEFAULT 0 CHECK (monthly_contribution >= 0),
    annual_return        numeric(6, 5) NOT NULL DEFAULT 0,
    current_balance      numeric(14, 2) NOT NULL DEFAULT 0,
    annual_salary        numeric(14, 2) NOT NULL DEFAULT 0,
    deferral_rate        numeric(6, 5) NOT NULL DEFAULT 0 CHECK (deferral_rate BETWEEN 0 AND 1),
    match_rate           numeric(6, 5) NOT NULL DEFAULT 0 CHECK (match_rate BETWEEN 0 AND 1),
    match_cap            numeric(6, 5) NOT NULL DEFAULT 0 CHECK (match_cap BETWEEN 0 AND 1),
    CHECK ((kind = 'rainy_day') = (months_of_cover > 0)),
    CHECK (kind = 'retirement' OR (annual_salary = 0 AND deferral_rate = 0))
);
COMMENT ON TABLE savings_goal IS
    'Savings goals. Every column is filled for every goal (zero where it does not apply), because an '
    'Improv formula over two measures only has cells where both have one. target_amount is a fixed '
    'target; a rainy-day goal instead covers months_of_cover months of essential budget. '
    'Payroll goals add deferral_rate x salary and the employer match. Every target date is a 30 June, '
    'so a target falls on a plan anniversary.';
COMMENT ON COLUMN savings_goal.current_balance IS
    'Value of the funding accounts at plan_settings.as_of (set from statements and holdings).';

CREATE TABLE goal_account (
    goal_id    text NOT NULL REFERENCES savings_goal,
    account_id text NOT NULL REFERENCES account,
    share      numeric(5, 4) NOT NULL CHECK (share > 0 AND share <= 1),
    PRIMARY KEY (goal_id, account_id)
);
COMMENT ON TABLE goal_account IS 'Which accounts fund a goal, and what share of each is earmarked for it.';

CREATE TABLE goal_horizon (
    goal_id        text NOT NULL REFERENCES savings_goal,
    year           text NOT NULL CHECK (year ~ '^[0-9]{4}$'),
    months_out     int NOT NULL CHECK (months_out > 0),
    growth_factor  float8 NOT NULL,   -- (1 + r/12)^months_out
    annuity_factor float8 NOT NULL,   -- ((1 + r/12)^months_out - 1) / (r/12), or months_out when r = 0
    is_target_year numeric(1) NOT NULL CHECK (is_target_year IN (0, 1)),
    PRIMARY KEY (goal_id, year)
);
COMMENT ON TABLE goal_horizon IS
    'Compounding factors for each goal at each plan anniversary up to its target date, monthly '
    'compounding and end-of-month contributions. Precomputed here because Improv has no POWER. '
    'Rebuilt by refresh_goal_horizon().';

CREATE TABLE goal_expense_window (
    goal_id text NOT NULL REFERENCES savings_goal,
    month   text NOT NULL REFERENCES period,
    weight  float8 NOT NULL CHECK (weight >= 0),
    PRIMARY KEY (goal_id, month)
);
COMMENT ON TABLE goal_expense_window IS
    'Averaging weights: a goal''s "monthly essential budget" is the weighted sum of these months'' '
    'essential budgets. Rows exist for every goal (Improv has no dimensionless results, so the '
    'average is computed per goal); goals with no months_of_cover ignore it.';

CREATE FUNCTION refresh_goal_horizon() RETURNS void LANGUAGE sql AS $$
    DELETE FROM finance.goal_horizon;
    INSERT INTO finance.goal_horizon
    SELECT g.goal_id,
           to_char(s.as_of + make_interval(years => k), 'YYYY'),
           12 * k,
           power(1 + g.annual_return / 12, 12 * k),
           CASE WHEN g.annual_return = 0 THEN 12 * k
                ELSE (power(1 + g.annual_return / 12, 12 * k) - 1) / (g.annual_return / 12) END,
           CASE WHEN s.as_of + make_interval(years => k) = g.target_date THEN 1 ELSE 0 END
      FROM finance.savings_goal g
     CROSS JOIN finance.plan_settings s
     CROSS JOIN LATERAL generate_series(1,
           (extract(year FROM g.target_date) - extract(year FROM s.as_of))::int) AS k;
$$;
