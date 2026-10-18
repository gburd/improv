//! FORECAST.ETS: LibreOffice's exponential smoothing
//! (sc/source/core/tool/interpr8.cxx, `ScETSForecastCalculation`), ported
//! step by step (its `KahanSum`s, its bisection searches for α, β and γ,
//! its quirks) so the results match LibreOffice's: Holt–Winters triple
//! smoothing with additive or multiplicative seasons, or double smoothing
//! (no season) when the seasonality is 0 or detected as 1.
//!
//! LibreOffice's names: `gamma` smooths the trend (Excel's β), `beta` the
//! season (Excel's γ); FORECAST.ETS.STAT's types follow Excel (2 is the
//! trend's, 3 the season's).

use super::Kahan;
use crate::builtins::{err, n, serial};
use crate::reduce::statistical::number;
use crate::reduce::Rows;
use crate::CellValue;
use chrono::{Datelike, NaiveDate};

/// Which function of the family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Add,
    Mult,
    PiAdd,
    PiMult,
    StatAdd,
    StatMult,
    Season,
}

/// The smallest change of α, β, γ the searches resolve.
const RES: f64 = 0.001;

/// Serial date `x` (truncated, as LibreOffice's `Date + sal_Int32`).
fn date(x: f64) -> Option<NaiveDate> {
    let base = NaiveDate::from_ymd_opt(1899, 12, 30)?.num_days_from_ce();
    NaiveDate::from_num_days_from_ce_opt(base.checked_add(x as i32)?)
}

/// The smoothing state over the prepared series.
struct Model {
    x: Vec<f64>,
    y: Vec<f64>,
    period: usize,
    step: f64,
    /// The day of the month of a monthly series (x counts months), or 0.
    month_day: u32,
    additive: bool,
    eds: bool,
    alpha: f64,
    beta: f64,
    gamma: f64,
    base: Vec<f64>,
    trend: Vec<f64>,
    per: Vec<f64>,
    fc: Vec<f64>,
    mae: f64,
    mase: f64,
    mse: f64,
    rmse: f64,
    smape: f64,
}

/// The value duplicates of one x aggregate to (Excel's aggregation codes).
fn aggregate(ys: &[f64], how: i32) -> f64 {
    match how {
        // ponytail: LibreOffice keeps the first value for AVERAGE (a bug);
        // Excel documents the average, which this is.
        1 => ys.iter().sum::<f64>() / ys.len() as f64,
        2 | 3 => ys.len() as f64,
        4 => ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        5 => {
            let mut s = ys.to_vec();
            s.sort_by(f64::total_cmp);
            let k = s.len();
            if k % 2 == 1 {
                s[k / 2]
            } else {
                (s[k / 2] + s[k / 2 - 1]) / 2.0
            }
        }
        6 => ys.iter().copied().fold(f64::INFINITY, f64::min),
        _ => ys.iter().sum(),
    }
}

impl Model {
    /// `PreprocessDataRange` and `initData`: sort by x, detect a monthly
    /// series, aggregate repeated x, find the step, fill gaps, find the
    /// period (`period` 1 = detect, 0 = none), prefill the state.
    fn new(
        rows: &Rows,
        period: usize,
        completion: bool,
        how: i32,
        target: Option<f64>,
        additive: bool,
    ) -> Result<Model, u8> {
        let mut pts = Vec::with_capacity(rows.len());
        for r in &rows.vals {
            let y = r.first().and_then(number).ok_or(err::VALUE)?;
            let x = r.get(1).and_then(number).ok_or(err::VALUE)?;
            pts.push((x, y));
        }
        if pts.is_empty() {
            return Err(err::VALUE);
        }
        pts.sort_by(|p, q| p.0.total_cmp(&q.0));
        let (mut x, mut y): (Vec<f64>, Vec<f64>) = pts.into_iter().unzip();
        // Excel: "If the target date is chronologically before the end of
        // the historical timeline, FORECAST.ETS returns the #NUM! error"
        // (LibreOffice interpolates within the data; its intervals agree).
        if target.is_some_and(|t| t < x[x.len() - 1]) {
            return Err(err::NUM);
        }
        // A monthly series: every date on the first's day of the month.
        let d0 = date(x[0]);
        let mut month_day = d0.map_or(0, |d| d.day());
        for xi in &x[1..] {
            if month_day == 0 {
                break;
            }
            let d1 = date(*xi);
            if d1 != d0 && d1.map(|d| d.day()) != Some(month_day) {
                month_day = 0;
            }
        }
        if month_day != 0 {
            for xi in &mut x {
                let d = date(*xi).ok_or(err::NUM)?;
                *xi = f64::from(d.year() * 12) + f64::from(d.month());
            }
        }
        let mut step = f64::MAX;
        let mut i = 1;
        while i < x.len() {
            let mut fstep = x[i] - x[i - 1];
            if fstep == 0.0 {
                let mut j = i;
                while j < x.len() && x[j] == x[i - 1] {
                    j += 1;
                }
                y[i - 1] = aggregate(&y[i - 1..j], how);
                x.drain(i..j);
                y.drain(i..j);
                fstep = if i + 1 < x.len() {
                    x[i] - x[i - 1]
                } else {
                    step
                };
            }
            if fstep > 0.0 && fstep < step {
                step = fstep;
            }
            i += 1;
        }
        // The step must be constant, or the first gap a multiple of it.
        let mut gap = false;
        for w in x.windows(2) {
            let f = w[1] - w[0];
            if f != step {
                if f % step != 0.0 {
                    return Err(err::VALUE);
                }
                gap = true;
                break;
            }
        }
        if gap {
            let orig = x.len() as f64;
            let mut missing = 0usize;
            // ponytail: LibreOffice measures a monthly gap on the month
            // counts read as serial dates (nonsense dates); kept, to match.
            let mut d0 = date(x[0]);
            let mut i = 1;
            while i < x.len() {
                let dist = if month_day != 0 {
                    let d1 = date(x[i]);
                    let months =
                        |d: Option<NaiveDate>| d.map_or(0, |d| d.year() * 12 + d.month() as i32);
                    let dist = f64::from(months(d1) - months(d0));
                    d0 = d1;
                    dist
                } else {
                    x[i] - x[i - 1]
                };
                if dist > step {
                    let fill = if completion {
                        (y[i] + y[i - 1]) / 2.0
                    } else {
                        0.0
                    };
                    let mut xg = Kahan::new(x[i - 1] + step);
                    while xg.get() < x[i] {
                        x.insert(i, xg.get());
                        y.insert(i, fill);
                        i += 1;
                        missing += 1;
                        if missing as f64 / orig > 0.3 {
                            return Err(err::VALUE);
                        }
                        xg.add(step);
                    }
                }
                i += 1;
            }
        }
        let len = x.len();
        let mut m = Model {
            x,
            y,
            period,
            step,
            month_day,
            additive,
            eds: period == 0,
            alpha: 0.0,
            beta: 0.0,
            gamma: 0.0,
            base: vec![0.0; len],
            trend: vec![0.0; len],
            per: vec![0.0; len],
            fc: vec![0.0; len],
            mae: 0.0,
            mase: 0.0,
            mse: 0.0,
            rmse: 0.0,
            smape: 0.0,
        };
        if period == 1 {
            m.period = m.period_len();
            m.eds = m.period == 1;
        }
        m.fc[0] = m.y[0];
        m.prefill()?;
        Ok(m)
    }

    /// `CalcPeriodLen`: the period (from n/2 down to 1) whose
    /// period-on-period changes differ least, the shorter on ties.
    fn period_len(&self) -> usize {
        let (n, y) = (self.y.len(), &self.y);
        let (mut best, mut best_err) = (n, f64::MAX);
        for p in (1..=n / 2).rev() {
            let periods = n / p;
            let mut s = Kahan::new(0.0);
            for i in n - periods * p + 1..n - p {
                s.add(((y[i] - y[i - 1]) - (y[p + i] - y[p + i - 1])).abs());
            }
            let e = s.get() / ((periods - 1) * p - 1) as f64;
            if e <= best_err || e == 0.0 {
                (best, best_err) = (p, e);
            }
        }
        best
    }

    /// `prefillTrendData`, `prefillPerIdx`, `prefillBaseData`.
    fn prefill(&mut self) -> Result<(), u8> {
        let (n, p) = (self.y.len(), self.period);
        let y = &self.y;
        if self.eds {
            self.trend[0] = (y[n - 1] - y[0]) / (n - 1) as f64;
            self.base[0] = y[0];
            return Ok(());
        }
        // Two periods at least.
        if n < 2 * p {
            return Err(err::VALUE);
        }
        let mut s = Kahan::new(0.0);
        for i in 0..p {
            s.add(y[i + p]);
            s.add(-y[i]);
        }
        let t0 = s.get() / (p * p) as f64;
        self.trend[0] = t0;
        let periods = n / p;
        let mut avg = Vec::with_capacity(periods);
        for i in 0..periods {
            let mut s = Kahan::new(0.0);
            for j in 0..p {
                s.add(y[i * p + j]);
            }
            let a = s.get() / p as f64;
            if a == 0.0 {
                return Err(err::DIV0);
            }
            avg.push(a);
        }
        for j in 0..p {
            let mut s = Kahan::new(0.0);
            for (i, a) in avg.iter().enumerate() {
                let level = a + (j as f64 - 0.5 * (p - 1) as f64) * t0;
                let v = y[i * p + j];
                s.add(if self.additive { v - level } else { v / level });
            }
            self.per[j] = s.get() / periods as f64;
        }
        if p < n {
            self.per[p] = 0.0;
        }
        // LibreOffice divides by the first index for additive seasons too.
        self.base[0] = y[0] / self.per[0];
        Ok(())
    }

    /// `calcAccuracyIndicators`.
    fn accuracy(&mut self) {
        let n = self.y.len();
        let (mut ae, mut se, mut pe, mut dv) = (
            Kahan::new(0.0),
            Kahan::new(0.0),
            Kahan::new(0.0),
            Kahan::new(0.0),
        );
        for i in 1..n {
            let e = self.fc[i] - self.y[i];
            ae.add(e.abs());
            se.add(e * e);
            pe.add(e.abs() / (self.fc[i].abs() + self.y[i].abs()));
        }
        for i in 2..n {
            dv.add((self.y[i] - self.y[i - 1]).abs());
        }
        let c = (n as f64) - 1.0;
        self.mae = ae.get() / c;
        self.mase = ae.get() / (c * dv.get() / (c - 1.0));
        self.mse = se.get() / c;
        self.rmse = self.mse.sqrt();
        self.smape = pe.get() * 2.0 / c;
    }

    /// `refill`: the one-step-ahead forecasts for the current α, β, γ.
    fn refill(&mut self) {
        let (n, p) = (self.y.len(), self.period);
        let (a, b, g) = (self.alpha, self.beta, self.gamma);
        for i in 1..n {
            let (b1, t1) = (self.base[i - 1], self.trend[i - 1]);
            if self.eds {
                self.base[i] = a * self.y[i] + (1.0 - a) * (b1 + t1);
                self.trend[i] = g * (self.base[i] - b1) + (1.0 - g) * t1;
                self.fc[i] = b1 + t1;
                continue;
            }
            let k;
            if self.additive {
                k = if i > p { i - p } else { i };
                self.base[i] = a * (self.y[i] - self.per[k]) + (1.0 - a) * (b1 + t1);
                self.per[i] = b * (self.y[i] - self.base[i]) + (1.0 - b) * self.per[k];
            } else {
                k = if i >= p { i - p } else { i };
                self.base[i] = a * (self.y[i] / self.per[k]) + (1.0 - a) * (b1 + t1);
                self.per[i] = b * (self.y[i] / self.base[i]) + (1.0 - b) * self.per[k];
            }
            self.trend[i] = g * (self.base[i] - b1) + (1.0 - g) * t1;
            self.fc[i] = if self.additive {
                b1 + t1 + self.per[k]
            } else {
                (b1 + t1) * self.per[k]
            };
        }
        self.accuracy();
    }

    /// LibreOffice's search of one parameter in [0, 1] (`CalcGamma` and
    /// its kin): bisect on the mean squared error, `inner` re-fitting the
    /// parameters below it.
    fn search(&mut self, set: fn(&mut Model, f64), inner: fn(&mut Model)) {
        let run = |m: &mut Model, v: f64| {
            set(m, v);
            inner(m);
        };
        let (mut f0, mut f1, mut f2) = (0.0, 0.5, 1.0);
        run(self, f0);
        let mut e0 = self.mse;
        run(self, f2);
        let mut e2 = self.mse;
        run(self, f1);
        if e0 == self.mse && self.mse == e2 {
            run(self, 0.0);
            return;
        }
        while f2 - f1 > RES {
            if e2 > e0 {
                f2 = f1;
                e2 = self.mse;
                f1 = (f0 + f1) / 2.0;
            } else {
                f0 = f1;
                e0 = self.mse;
                f1 = (f1 + f2) / 2.0;
            }
            run(self, f1);
        }
        if e2 > e0 {
            if e0 < self.mse {
                run(self, f0);
            }
        } else if e2 < self.mse {
            run(self, f2);
        }
    }

    fn fit_gamma(&mut self) {
        self.search(|m, v| m.gamma = v, Model::refill);
    }

    fn fit_beta_gamma(&mut self) {
        self.search(
            |m, v| m.beta = v,
            |m| {
                m.fit_gamma();
                m.refill();
            },
        );
    }

    /// `CalcAlphaBetaGamma` (`initCalc`).
    fn fit(&mut self) {
        self.search(
            |m, v| m.alpha = v,
            |m| {
                if m.eds {
                    m.fit_gamma();
                } else {
                    m.fit_beta_gamma();
                }
                m.refill();
            },
        );
        self.accuracy();
    }

    /// `convertXtoMonths`: a serial date as fractional months.
    fn months(&self, t: f64) -> Result<f64, u8> {
        let d = date(t).ok_or(err::NUM)?;
        let len = match d.month() {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31.0,
            2 if d.leap_year() => 29.0,
            2 => 28.0,
            _ => 30.0,
        };
        Ok(12.0 * f64::from(d.year())
            + f64::from(d.month())
            + (f64::from(d.day()) - f64::from(self.month_day)) / len)
    }

    /// The target on the series' axis (months for a monthly series).
    fn axis(&self, t: f64) -> Result<f64, u8> {
        if self.month_day != 0 {
            self.months(t)
        } else {
            Ok(t)
        }
    }

    /// `GetForecast` past the data (t ≥ the last x; at it, its value): the
    /// smoothed level, trend and season, interpolated between steps.
    fn forecast(&self, t: f64) -> f64 {
        let n = self.y.len();
        let at = |v: &[f64], k: usize| v.get(k).copied().unwrap_or(f64::NAN);
        let last = self.x[n - 1];
        if t <= last {
            return self.y[n - 1];
        }
        let k = ((t - last) / self.step) as usize;
        let frac = (t - last) % self.step;
        let ahead = |k: usize| {
            let lvl = self.base[n - 1] + k as f64 * self.trend[n - 1];
            if self.eds {
                return lvl;
            }
            let s = at(
                &self.per,
                (n - 1 + k % self.period).wrapping_sub(self.period),
            );
            if self.additive {
                lvl + s
            } else {
                lvl * s
            }
        };
        let r = ahead(k);
        if frac >= RES {
            return r + frac / self.step * (ahead(k + 1) - r);
        }
        r
    }

    /// `GetEDSPredictionIntervals` for one target: the half-width of the
    /// prediction interval at `level` (double smoothing only).
    fn interval(&self, t: f64, level: f64) -> Result<f64, u8> {
        let ahead = self.axis(t)? - self.x[self.x.len() - 1];
        let mut size = (ahead / self.step) as usize;
        if ahead % self.step != 0.0 {
            size += 1;
        }
        if size == 0 {
            return Err(err::NUM);
        }
        let z = super::normality::qnorm((1.0 + level) / 2.0);
        let o = 1.0 - level;
        let c = |i: usize| {
            let i = i as f64;
            (1.0 + (level / (1.0 + o).powf(3.0))
                * ((1.0 + 4.0 * o + 5.0 * o * o)
                    + 2.0 * i * level * (1.0 + 3.0 * o)
                    + 2.0 * (i * i) * level * level))
                .sqrt()
        };
        // As LibreOffice: whole steps ahead less one, interpolated by the
        // remainder unscaled (its quirks). A target less than a step ahead
        // reads past LibreOffice's coefficients (undefined); here c
        // continues its formula.
        let steps = (ahead / self.step - 1.0) as usize;
        let frac = ahead % self.step;
        let mut pi = z * self.rmse * c(steps) / c(0);
        if frac != 0.0 {
            let pi1 = z * self.rmse * c(steps + 1) / c(0);
            pi += frac * (pi1 - pi);
        }
        Ok(pi)
    }

    /// `GetStatisticValue`.
    fn stat(&self, kind: i32) -> f64 {
        match kind {
            1 => self.alpha,
            2 => self.gamma,
            3 => self.beta,
            4 => self.mase,
            5 => self.smape,
            6 => self.mae,
            7 => self.rmse,
            8 => self.step,
            _ => self.period as f64,
        }
    }
}

/// A target: a date as its serial, else a number.
fn serial_of(a: &[CellValue], i: usize) -> Result<f64, u8> {
    match a.get(i) {
        Some(CellValue::Date(ms)) => Ok(serial(*ms)),
        _ => n(a, i),
    }
}

/// Optional parameter `i`, or `default`.
fn opt(a: &[CellValue], i: usize, default: f64) -> Result<f64, u8> {
    if a.len() > i {
        n(a, i)
    } else {
        Ok(default)
    }
}

/// Evaluate `kind` over values and timeline (`rows`: [values, timeline])
/// with its parameters, in LibreOffice's order of checks.
pub(super) fn ets(kind: Kind, rows: &Rows, a: &[CellValue]) -> Result<f64, u8> {
    use Kind::*;
    // (period, completion, aggregation) parameter positions.
    let (pi, ci, ai) = match kind {
        Add | Mult | StatAdd | StatMult => (1, 2, 3),
        PiAdd | PiMult => (2, 3, 4),
        Season => (usize::MAX, 0, 1),
    };
    let how = opt(a, ai, 1.0)? as i32;
    if !(1..=7).contains(&how) {
        return Err(err::NUM);
    }
    let completion = match opt(a, ci, 1.0)? as i32 {
        0 => false,
        1 => true,
        _ => return Err(err::NUM),
    };
    let period = if kind == Season {
        1
    } else {
        let v = opt(a, pi, 1.0)?;
        if v % 1.0 != 0.0 || v < 0.0 {
            return Err(err::NUM);
        }
        v as usize
    };
    let level = if matches!(kind, PiAdd | PiMult) {
        let l = opt(a, 1, 0.95)?;
        if !(0.0..=1.0).contains(&l) {
            return Err(err::NUM);
        }
        l
    } else {
        0.0
    };
    let stat = if matches!(kind, StatAdd | StatMult) {
        let s = n(a, 0)? as i32;
        if !(1..=9).contains(&s) {
            return Err(err::NUM);
        }
        s
    } else {
        0
    };
    let target = match kind {
        Add | Mult | PiAdd | PiMult => Some(serial_of(a, 0)?),
        _ => None,
    };
    let additive = matches!(kind, Add | PiAdd | StatAdd);
    let mut m = Model::new(rows, period, completion, how, target, additive)?;
    if kind == Season {
        return Ok(m.period as f64);
    }
    m.fit();
    match kind {
        Add | Mult => Ok(m.forecast(m.axis(target.unwrap_or_default())?)),
        // ponytail: LibreOffice simulates a seasonal model's interval with
        // random scenarios (not reproducible); only double smoothing's
        // closed form (seasonality 0) is supported.
        PiAdd | PiMult if period != 0 => Err(err::NA),
        PiAdd | PiMult => m.interval(target.unwrap_or_default(), level),
        _ => Ok(m.stat(stat)),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn rows(ys: &[f64], xs: &[f64]) -> Rows {
        Rows {
            pos: (0..ys.len() as u32).map(|i| vec![i]).collect(),
            vals: ys
                .iter()
                .zip(xs)
                .map(|(y, x)| vec![CellValue::num(*y), CellValue::num(*x)])
                .collect(),
            capacity: ys.len(),
        }
    }

    fn num(xs: &[f64]) -> Vec<CellValue> {
        xs.iter().map(|x| CellValue::num(*x)).collect()
    }

    #[test]
    fn average_aggregates_duplicates() {
        // Excel's AVERAGE: the duplicates' mean (LibreOffice keeps the first).
        let ys = [3.0, 5.0, 4.0, 6.0, 8.0, 7.0, 9.0, 11.0];
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let plain = ets(Kind::Add, &rows(&ys, &xs), &num(&[10.0, 0.0])).unwrap();
        let mut dy = ys.to_vec();
        let mut dx = xs.to_vec();
        dy.splice(2..3, [3.0, 5.0]);
        dx.splice(2..3, [3.0, 3.0]);
        let dup = ets(Kind::Add, &rows(&dy, &dx), &num(&[10.0, 0.0])).unwrap();
        assert_eq!(plain, dup);
        assert_eq!(aggregate(&[1.0, 4.0, 2.0, 3.0], 5), 2.5);
        assert_eq!(aggregate(&[1.0, 4.0, 2.0], 5), 2.0);
        // Repeated x at the end of the series.
        let mut ey = ys.to_vec();
        let mut ex = xs.to_vec();
        ey.push(13.0);
        ex.push(8.0);
        assert!(ets(Kind::Add, &rows(&ey, &ex), &num(&[10.0, 0.0])).is_ok());
    }

    #[test]
    fn parameters_and_shapes() {
        let ys = [3.0, 5.0, 4.0, 6.0, 8.0, 7.0, 9.0, 11.0];
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let r = rows(&ys, &xs);
        // A seasonal interval is not supported: #N/A.
        assert_eq!(ets(Kind::PiAdd, &r, &num(&[10.0])), Err(err::NA));
        assert_eq!(ets(Kind::PiAdd, &r, &num(&[10.0, 1.5])), Err(err::NUM));
        // The interval at the last point is empty: #NUM!; before it, and
        // a forecast before it (Excel), too.
        assert_eq!(ets(Kind::PiAdd, &r, &num(&[8.0, 0.9, 0.0])), Err(err::NUM));
        assert_eq!(ets(Kind::Add, &r, &num(&[7.5])), Err(err::NUM));
        assert_eq!(ets(Kind::Add, &r, &num(&[8.0])), Ok(11.0));
        assert_eq!(ets(Kind::StatAdd, &r, &num(&[0.0])), Err(err::NUM));
        // Less than a step ahead on a step of 2: c(1)/c(0), the half-width
        // two steps ahead (LibreOffice reads past its array here).
        let x2: Vec<f64> = xs.iter().map(|x| 2.0 * x).collect();
        let r2 = rows(&ys, &x2);
        assert_eq!(
            ets(Kind::PiAdd, &r2, &num(&[17.0, 0.95, 0.0])),
            ets(Kind::PiAdd, &r2, &num(&[20.0, 0.95, 0.0]))
        );
        assert_eq!(
            ets(Kind::Add, &r, &[CellValue::Text("x".into())]),
            Err(err::VALUE)
        );
        assert_eq!(
            ets(Kind::Add, &rows(&[], &[]), &num(&[1.0])),
            Err(err::VALUE)
        );
        let mut t = rows(&ys, &xs);
        t.vals[1][1] = CellValue::Text("x".into());
        assert_eq!(ets(Kind::Add, &t, &num(&[9.0])), Err(err::VALUE));
        // A date target is its serial (day 9 of the series' numbers).
        let day9 = CellValue::Date(-2_209_161_600_000 + 9 * 86_400_000);
        assert_eq!(
            ets(Kind::Add, &r, &[day9, CellValue::num(0.0)]),
            ets(Kind::Add, &r, &num(&[9.0, 0.0]))
        );
        // Too many gaps (over 30% filled in).
        let gx = [1.0, 2.0, 6.0, 7.0, 8.0, 9.0];
        assert_eq!(
            ets(Kind::Add, &rows(&ys[..6], &gx), &num(&[10.0])),
            Err(err::VALUE)
        );
        // A zero season average.
        let z = rows(&[0.0, 0.0, 1.0, -1.0], &xs[..4]);
        assert_eq!(ets(Kind::Mult, &z, &num(&[5.0, 2.0])), Err(err::DIV0));
        assert!(date(1e12).is_none());
    }
}
