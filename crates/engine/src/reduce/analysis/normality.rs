//! Gnumeric's tests of normality (plugins/fn-stat, src/rangefunc.c), ported
//! line by line: Anderson–Darling, Cramér–von Mises, Lilliefors
//! (Kolmogorov–Smirnov) and Shapiro–Francia. Gnumeric returns {p,
//! statistic, n}; `ADTEST` and friends return p, `ADTEST.STAT` and friends
//! the statistic. Non-numbers are skipped.

use crate::builtins::statistical::norm_cdf;
use crate::builtins::{self, err};
use crate::reduce::statistical::number;
use crate::reduce::Rows;
use crate::CellValue;
use improv_core_model::FuncId;

/// Which test.
#[derive(Clone, Copy)]
pub(super) enum Test {
    Ad,
    Cvm,
    Lks,
    Sf,
}

/// Φ⁻¹(p), the registry's `NORM.S.INV`.
pub(super) fn qnorm(p: f64) -> f64 {
    builtins::eval(FuncId(529), &[CellValue::num(p)])
        .as_num()
        .unwrap_or(f64::NAN)
}

/// The mean (exact for constant data, as Gnumeric's) and sample standard
/// deviation.
fn mean_sd(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mu = if xs.iter().all(|x| *x == xs[0]) {
        xs[0]
    } else {
        xs.iter().sum::<f64>() / n
    };
    let ss: f64 = xs.iter().map(|x| (x - mu) * (x - mu)).sum();
    (mu, (ss / (n - 1.0)).sqrt())
}

/// (p, statistic) of `test` over the group's numbers. Too few values (8
/// for Anderson–Darling and Cramér–von Mises, 5 for the others; Shapiro–
/// Francia at most 5000) is #VALUE!. Constant data, as in Gnumeric: the
/// fitted normal is a step at the mean (R's `pnorm` with σ = 0), so
/// Cramér–von Mises and Lilliefors reject (p ≈ 0), Anderson–Darling's
/// statistic is infinite (#NUM!) and Shapiro–Francia's correlation
/// undefined (#VALUE!).
pub(super) fn normality(rows: &Rows, test: Test) -> Result<(f64, f64), u8> {
    let mut ys: Vec<f64> = rows.col(0).filter_map(number).collect();
    ys.sort_by(f64::total_cmp);
    let len = ys.len();
    let n = len as f64;
    let min = match test {
        Test::Ad | Test::Cvm => 8,
        Test::Lks | Test::Sf => 5,
    };
    if len < min || (matches!(test, Test::Sf) && len > 5000) {
        return Err(err::VALUE);
    }
    let (mu, sigma) = mean_sd(&ys);
    if sigma == 0.0 && matches!(test, Test::Sf) {
        return Err(err::VALUE);
    }
    let z = |y: f64| match (sigma == 0.0, y < mu) {
        (true, true) => f64::NEG_INFINITY,
        (true, false) => f64::INFINITY,
        _ => (y - mu) / sigma,
    };
    Ok(match test {
        Test::Ad => {
            let ln_lower = |y: f64| (0.5 * libm::erfc(-z(y) / std::f64::consts::SQRT_2)).ln();
            let ln_upper = |y: f64| (0.5 * libm::erfc(z(y) / std::f64::consts::SQRT_2)).ln();
            let mut total = 0.0;
            for i in 0..len {
                total += (2 * i + 1) as f64 * (ln_lower(ys[i]) + ln_upper(ys[len - i - 1]));
            }
            let mut a = -n - total / n;
            a *= 1.0 + 0.75 / n + 2.25 / (n * n);
            let p = if a < 0.2 {
                -(-13.436 + 101.14 * a - 223.73 * a * a).exp_m1()
            } else if a < 0.34 {
                -(-8.318 + 42.796 * a - 59.938 * a * a).exp_m1()
            } else if a < 0.6 {
                (0.9177 - 4.279 * a - 1.38 * a * a).exp()
            } else {
                (1.2937 - 5.709 * a + 0.0186 * a * a).exp()
            };
            (p, a)
        }
        Test::Cvm => {
            let mut w: f64 = (0..len)
                .map(|i| {
                    let d = norm_cdf(z(ys[i])) - (2 * i + 1) as f64 / (2.0 * n);
                    d * d
                })
                .sum();
            w += 1.0 / (12.0 * n);
            let stat = w;
            w *= 1.0 + 0.5 / n;
            let p = if w < 0.0275 {
                1.0 - (-13.953 + 775.5 * w - 12542.61 * w * w).exp()
            } else if w < 0.051 {
                1.0 - (-5.903 + 179.546 * w - 1515.29 * w * w).exp()
            } else if w < 0.092 {
                (0.886 - 31.62 * w - 10.897 * w * w).exp()
            } else if w < 1.0 {
                (1.111 - 34.242 * w + 12.832 * w * w).exp()
            } else {
                0.0
            };
            (p, stat)
        }
        Test::Lks => {
            let val = norm_cdf(z(ys[0]));
            let (mut dplus, mut dminus) = (1.0 / n - val, val);
            for (i, y) in ys.iter().enumerate().skip(1) {
                let val = norm_cdf(z(*y));
                dplus = dplus.max((i + 1) as f64 / n - val);
                dminus = dminus.max(val - i as f64 / n);
            }
            let d = if dplus < dminus { dminus } else { dplus };
            let (mut s, size) = if len > 100 {
                (d * (n / 100.0).powf(0.49), 100.0)
            } else {
                (d, n)
            };
            let mut p = (-7.01256 * s * s * (size + 2.78019)
                + 2.99587 * s * (size + 2.78019).sqrt()
                - 0.122119
                + 0.974598 / size.sqrt()
                + 1.67997 / size)
                .exp();
            if p > 0.1 {
                s *= size.sqrt() - 0.01 + 0.85 / size.sqrt();
                let poly = |c: [f64; 5]| c[0] + s * (c[1] + s * (c[2] + s * (c[3] + s * c[4])));
                // p > 0.1 bounds the rescaled s below 0.84 for every size
                // from 5 to 100, so nortest's bands above 0.9 (Gnumeric's
                // 0.9 < s ≤ 1.31 polynomial, and 0 beyond) never apply.
                p = if s <= 0.302 {
                    1.0
                } else if s <= 0.5 {
                    poly([2.76773, -19.828315, 80.709644, -138.55152, 81.218052])
                } else {
                    poly([-4.901232, 40.662806, -97.490286, 94.029866, -32.355711])
                };
            }
            (p, d)
        }
        Test::Sf => {
            let zs: Vec<f64> = (1..=len)
                .map(|i| qnorm((i as f64 - 3.0 / 8.0) / (n + 0.25)))
                .collect();
            let (mz, sz) = mean_sd(&zs);
            let cov: f64 = ys
                .iter()
                .zip(&zs)
                .map(|(y, z)| (y - mu) * (z - mz))
                .sum::<f64>()
                / n;
            // Population standard deviations: the sample ones scaled.
            let pop = ((n - 1.0) / n).sqrt();
            let r = (cov / (sigma * pop * sz * pop)).clamp(-1.0, 1.0);
            let w = r * r;
            let u = n.ln();
            let v = u.ln();
            let m = -1.2725 + 1.0521 * (v - u);
            let s = 1.0308 - 0.26758 * (v + 2.0 / u);
            // The upper tail of N(m, s) at ln(1 − W).
            let p = 1.0 - norm_cdf(((-w).ln_1p() - m) / s);
            (p, w)
        }
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn rows(xs: &[f64]) -> Rows {
        Rows {
            pos: (0..xs.len() as u32).map(|i| vec![i]).collect(),
            vals: xs.iter().map(|x| vec![CellValue::num(*x)]).collect(),
            capacity: xs.len(),
        }
    }

    #[test]
    fn sizes_and_constants() {
        let few = rows(&[1.0, 2.0, 3.0, 4.0]);
        for t in [Test::Ad, Test::Cvm, Test::Lks, Test::Sf] {
            assert_eq!(normality(&few, t), Err(err::VALUE));
        }
        let flat = rows(&[2.0; 9]);
        assert!(!normality(&flat, Test::Ad).unwrap().1.is_finite());
        assert_eq!(normality(&flat, Test::Sf), Err(err::VALUE));
        // A step below the mean of a constant: one value below.
        let step = rows(&[1.0, 2.0, 2.0, 2.0, 2.0]);
        assert!(normality(&step, Test::Lks).is_ok());
        assert_eq!(normality(&rows(&[0.0; 5001]), Test::Sf), Err(err::VALUE));
        // More than 100 values: Lilliefors rescales its statistic.
        let many: Vec<f64> = (0..150).map(|i| f64::from(i * i % 97)).collect();
        let (p, d) = normality(&rows(&many), Test::Lks).unwrap();
        assert!((0.0..=1.0).contains(&p) && d > 0.0);
        // Every p-value band of the four tests: samples from normal to
        // heavily skewed.
        let mut bands = std::collections::BTreeSet::new();
        for k in 0..60 {
            let xs: Vec<f64> = (1..=20)
                .map(|i| {
                    let u = (f64::from(i) - 0.5) / 20.0;
                    qnorm(u) + f64::from(k) * 0.05 * (3.0 * u).exp()
                })
                .collect();
            for (t, name) in [(Test::Ad, "ad"), (Test::Cvm, "cvm"), (Test::Lks, "lks")] {
                let (p, _) = normality(&rows(&xs), t).unwrap();
                bands.insert((name, (p * 10.0) as u8));
            }
        }
        assert!(bands.len() > 12, "{bands:?}");
        // A wild outlier: p = 0 in Cramér–von Mises's last branch.
        let mut out = vec![0.0; 40];
        out[39] = 1e6;
        out[38] = 1.0;
        assert_eq!(normality(&rows(&out), Test::Cvm).unwrap().0, 0.0);
    }

    /// The bound behind Lilliefors' two-band rescale: wherever the first
    /// approximation's p exceeds 0.1, the rescaled statistic stays ≤ 0.9.
    #[test]
    fn lilliefors_rescale_stays_below_the_upper_bands() {
        for size in 5..=100 {
            let size = f64::from(size);
            let p = |s: f64| {
                (-7.01256 * s * s * (size + 2.78019) + 2.99587 * s * (size + 2.78019).sqrt()
                    - 0.122119
                    + 0.974598 / size.sqrt()
                    + 1.67997 / size)
                    .exp()
            };
            // Bisect for the largest s with p(s) > 0.1 (p falls past its peak).
            let (mut lo, mut hi) = (0.05, 1.0);
            for _ in 0..60 {
                let mid = (lo + hi) / 2.0;
                if p(mid) > 0.1 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let scaled = hi * (size.sqrt() - 0.01 + 0.85 / size.sqrt());
            assert!(scaled < 0.85, "size {size}: {scaled}");
        }
    }

    /// LKSTEST p and statistic against Gnumeric 1.12 for 150 values
    /// (rescaled to size 100), in the 0.5 < s ≤ 0.9 band.
    #[test]
    fn lilliefors_matches_gnumeric() {
        let xs: Vec<f64> = (1..=150)
            .map(|i| {
                let u = (f64::from(i) - 0.5) / 150.0;
                qnorm(u) + 0.06 * (3.0 * u).exp()
            })
            .collect();
        let (p, d) = normality(&rows(&xs), Test::Lks).unwrap();
        assert!((p - 0.855236065705248).abs() < 1e-12, "{p}");
        assert!((d - 0.03825606949390914).abs() < 1e-14, "{d}");
    }
}
