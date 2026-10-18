//! Evaluation of the P7 engineering functions (ids 1300–1449): base
//! conversion and bit operations (Excel's rules), complex numbers as
//! `"a+bi"` text, Bessel functions and CONVERT's unit table.

use super::{domain, err, n, n_or, ok_num, ok_text, t, R};
use crate::CellValue;
use improv_verified::{bits as vbits, radix};

mod complex;
mod convert;

/// 2⁴⁸: the bit functions' exclusive bound.
const BIT_LIMIT: f64 = 281_474_976_710_656.0;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    let x = |i: usize| n(a, i);
    let places = |i: usize| {
        if a.len() > i {
            n(a, i).map(Some)
        } else {
            Ok(None)
        }
    };
    match id {
        // --- bases ---
        1300 => ok_num(from_base(a, 2)? as f64),
        1301 => to_base(from_base(a, 2)?, 16, places(1)?),
        1302 => to_base(from_base(a, 2)?, 8, places(1)?),
        1303 => to_base(dec(x(0)?)?, 2, places(1)?),
        1304 => to_base(dec(x(0)?)?, 16, places(1)?),
        1305 => to_base(dec(x(0)?)?, 8, places(1)?),
        1306 => to_base(from_base(a, 16)?, 2, places(1)?),
        1307 => ok_num(from_base(a, 16)? as f64),
        1308 => to_base(from_base(a, 16)?, 8, places(1)?),
        1309 => to_base(from_base(a, 8)?, 2, places(1)?),
        1310 => ok_num(from_base(a, 8)? as f64),
        1311 => to_base(from_base(a, 8)?, 16, places(1)?),
        1312 => base(x(0)?, x(1)?, n_or(a, 2, 0.0)?),
        1313 => decimal(&t(a, 0)?, x(1)?),
        1314 => ok_text(
            x(0)?
                .to_le_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        ),
        // --- bits ---
        1315 => ok_num(vbits::and(bits(x(0)?)?, bits(x(1)?)?) as f64),
        1316 => ok_num(vbits::or(bits(x(0)?)?, bits(x(1)?)?) as f64),
        1317 => ok_num(vbits::xor(bits(x(0)?)?, bits(x(1)?)?) as f64),
        1318 => shift(x(0)?, x(1)?.floor()),
        1319 => shift(x(0)?, -x(1)?.floor()),
        // --- tests ---
        1320 => ok_num(f64::from(u8::from(x(0)? == n_or(a, 1, 0.0)?))),
        1321 => ok_num(f64::from(u8::from(x(0)? >= n_or(a, 1, 0.0)?))),
        1330..=1374 => complex::eval(id, a),
        1390 => convert::eval(a),
        1380..=1383 => {
            let (v, order) = (x(0)?, x(1)?.trunc());
            // Every method here walks the orders one at a time (musl's
            // recurrences, I's leading term, K's upward recurrence), so the
            // order is capped: 1e7 takes about 0.02 s, 2³¹ took 4 to 14 s.
            // LibreOffice caps BESSELJ's work at 9e6 steps the same way.
            domain((0.0..=BESSEL_MAX_ORDER).contains(&order))?;
            let k = order as i32;
            match id {
                1380 => ok_num(bessel_i(v, k)),
                1381 => ok_num(libm::jn(k, v)),
                1382 => {
                    domain(v > 0.0)?;
                    ok_num(bessel_k(v, k))
                }
                _ => {
                    domain(v > 0.0)?;
                    ok_num(libm::yn(k, v))
                }
            }
        }
        _ => Err(err::NAME),
    }
}

// --- bases ---------------------------------------------------------------

/// Argument 0 as digits in `base`: at most 10 of them; 10 digits whose
/// first is at least `base / 2` are negative (10-digit two's complement).
/// Empty text is 0. The arithmetic is [`radix::decode`], proven.
fn from_base(a: &[CellValue], base: u32) -> Result<i64, u8> {
    if matches!(a.first(), Some(CellValue::Bool(_))) {
        return Err(err::VALUE);
    }
    let s = t(a, 0)?;
    domain(s.chars().count() <= 10)?;
    // Digit values 0–35; one not below `base` is rejected by `decode`.
    let ds = s
        .chars()
        .map(|c| c.to_digit(36).map(|d| d as u8).ok_or(err::NUM))
        .collect::<Result<Vec<u8>, u8>>()?;
    radix::decode(&ds, base).ok_or(err::NUM)
}

/// Digit values (each below `base`, as [`radix`] proves) as uppercase text.
fn digit_text(ds: &[u8], base: u32) -> String {
    ds.iter()
        .map(|&d| char::from_digit(u32::from(d), base).map_or('0', |c| c.to_ascii_uppercase()))
        .collect()
}

/// A decimal argument, truncated (its range is checked by [`to_base`]).
fn dec(x: f64) -> Result<i64, u8> {
    let x = x.trunc();
    domain(x.abs() < 1e12)?;
    Ok(x as i64)
}

/// `v` as text in `base` (2, 8 or 16), which must hold it in 10 digits of
/// two's complement. A non-negative `v` is zero-padded to `places` (1–10,
/// truncated); a negative one is always 10 digits.
fn to_base(v: i64, base: u32, places: Option<f64>) -> R {
    // Proven: `None` exactly out of range, else ≤ 10 digits that decode to `v`.
    let ds = radix::encode(v, base).ok_or(err::NUM)?;
    let places = match places {
        Some(p) => {
            let p = p.trunc();
            domain((1.0..=10.0).contains(&p))?;
            p as usize
        }
        None => 0,
    };
    let digits = digit_text(&ds, base);
    if v < 0 {
        return ok_text(digits);
    }
    domain(digits.len() <= places || places == 0)?;
    ok_text(format!("{digits:0>places$}"))
}

/// The radix argument of BASE / DECIMAL: 2…36, truncated.
fn radix(r: f64) -> Result<u32, u8> {
    let r = r.trunc();
    domain((2.0..=36.0).contains(&r))?;
    Ok(r as u32)
}

/// BASE: a non-negative integer below 2⁵³ in `radix`, at least `min_len`
/// (0…255) digits.
fn base(x: f64, r: f64, min_len: f64) -> R {
    let (x, r, min_len) = (x.trunc(), radix(r)?, min_len.trunc());
    domain((0.0..9_007_199_254_740_992.0).contains(&x) && (0.0..=255.0).contains(&min_len))?;
    let s = digit_text(&radix::to_digits(x as u64, r), r);
    let width = min_len as usize;
    ok_text(format!("{s:0>width$}"))
}

/// DECIMAL: digits in `radix` (either case, at most 255) as a number.
fn decimal(s: &str, r: f64) -> R {
    let r = radix(r)?;
    domain(s.chars().count() <= 255)?;
    let mut v = 0.0;
    for c in s.chars() {
        v = v * f64::from(r) + f64::from(c.to_digit(r).ok_or(err::NUM)?);
    }
    ok_num(v)
}

// --- bits ----------------------------------------------------------------

/// A bit-function operand: truncated, in 0…2⁴⁸−1.
fn bits(x: f64) -> Result<u64, u8> {
    let x = x.trunc();
    domain((0.0..BIT_LIMIT).contains(&x))?;
    Ok(x as u64)
}

/// `x` shifted left by `by` (an integer) bits, right if negative, |by| ≤ 53.
/// A fractional shift is floored first, as LibreOffice does.
fn shift(x: f64, by: f64) -> R {
    let x = bits(x)?;
    domain(by.abs() <= 53.0)?;
    // Proven exact (`x · 2ⁿ < 2¹⁰¹`, so exact in f64 too: ≤ 48 significant
    // bits) and in range.
    if by >= 0.0 {
        ok_num(vbits::shl(x, by as u32) as f64)
    } else {
        ok_num(vbits::shr(x, (-by) as u32) as f64)
    }
}

// --- Bessel ----------------------------------------------------------------
// J and Y are musl's `jn` / `yn`. I and K are LibreOffice's scaddins
// (bessel.cxx), which is Excel's algorithm: I sums its power series, K uses
// Abramowitz & Stegun's polynomial fits for K₀ and K₁ and the upward
// recurrence.
// ponytail: K's fits are good to about 1e-8 relative, as in Excel and
// LibreOffice; port a Temme / continued-fraction K if more is ever wanted.

/// The largest Bessel order BESSELI … BESSELY take (#NUM! past it).
// ponytail: past it J and Y could use the asymptotic (Debye) forms and I
// and K ratios of their uniform expansions; nobody asks for those orders.
const BESSEL_MAX_ORDER: f64 = 1e7;

/// Iₙ(x) = Σₖ (x/2)^(n+2k) / (k! (n+k)!), to 1e-15 relative or 2000 terms.
fn bessel_i(x: f64, n: i32) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0;
    for k in 1..=n {
        term = term / f64::from(k) * half;
    }
    let mut sum = term;
    if term != 0.0 {
        let mut k = 1;
        loop {
            term = term * half / f64::from(k) * half / f64::from(k + n);
            sum += term;
            k += 1;
            if term.abs() <= sum.abs() * 1e-15 || k >= 2000 {
                break;
            }
        }
    }
    sum
}

// The coefficients are Abramowitz & Stegun 9.8.5/9.8.6's fit as Excel and
// LibreOffice use it; -0.57721566 is the fit's, not γ to full precision.
#[allow(clippy::approx_constant)]
fn bessel_k0(x: f64) -> f64 {
    if x <= 2.0 {
        let h = x * 0.5;
        let y = h * h;
        -h.ln() * bessel_i(x, 0)
            + (-0.577_215_66
                + y * (0.422_784_20
                    + y * (0.230_697_56
                        + y * (0.348_859_0e-1
                            + y * (0.262_698e-2 + y * (0.107_50e-3 + y * 0.74e-5))))))
    } else {
        let y = 2.0 / x;
        (-x).exp() / x.sqrt()
            * (1.253_314_14
                + y * (-0.783_235_8e-1
                    + y * (0.218_956_8e-1
                        + y * (-0.106_244_6e-1
                            + y * (0.587_872e-2 + y * (-0.251_540e-2 + y * 0.532_08e-3))))))
    }
}

fn bessel_k1(x: f64) -> f64 {
    if x <= 2.0 {
        let h = x * 0.5;
        let y = h * h;
        h.ln() * bessel_i(x, 1)
            + (1.0
                + y * (0.154_431_44
                    + y * (-0.672_785_79
                        + y * (-0.181_568_97
                            + y * (-0.191_940_2e-1 + y * (-0.110_404e-2 + y * -0.468_6e-4))))))
                / x
    } else {
        let y = 2.0 / x;
        (-x).exp() / x.sqrt()
            * (1.253_314_14
                + y * (0.234_986_19
                    + y * (-0.365_562_0e-1
                        + y * (0.150_426_8e-1
                            + y * (-0.780_353e-2 + y * (0.325_614e-2 + y * -0.682_45e-3))))))
    }
}

/// Kₙ(x), x > 0: K₀, K₁ and Kₙ₊₁ = Kₙ₋₁ + (2n/x) Kₙ.
fn bessel_k(x: f64, n: i32) -> f64 {
    match n {
        0 => bessel_k0(x),
        1 => bessel_k1(x),
        _ => {
            let tox = 2.0 / x;
            let (mut km, mut k) = (bessel_k0(x), bessel_k1(x));
            for j in 1..n {
                let kp = km + f64::from(j) * tox * k;
                km = k;
                k = kp;
            }
            k
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn call(id: u32, args: &[CellValue]) -> CellValue {
        eval(id, args).unwrap_or_else(CellValue::Err)
    }

    fn txt(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }

    fn num(x: f64) -> CellValue {
        CellValue::num(x)
    }

    /// The branches the goldens do not reach.
    #[test]
    fn base_edges() {
        let e = CellValue::Err;
        assert_eq!(call(1300, &[CellValue::Bool(true)]), e(err::VALUE));
        assert_eq!(call(1300, &[txt("")]), num(0.0));
        assert_eq!(call(1300, &[num(101.0)]), num(5.0));
        assert_eq!(call(1307, &[txt("ff")]), num(255.0));
        // places: too small, too large, negative numbers ignore a valid one
        assert_eq!(call(1303, &[num(4.0), num(2.0)]), e(err::NUM));
        assert_eq!(call(1303, &[num(4.0), num(11.0)]), e(err::NUM));
        assert_eq!(call(1303, &[num(-4.0), num(3.0)]), txt("1111111100"));
        assert_eq!(call(1303, &[num(1e13)]), e(err::NUM));
        assert_eq!(call(1312, &[num(255.0), num(16.0)]), txt("FF"));
        assert_eq!(call(1312, &[num(0.0), num(2.0)]), txt("0"));
        assert_eq!(call(1312, &[num(1.0), num(2.0), num(256.0)]), e(err::NUM));
        assert_eq!(call(1313, &[txt("zz"), num(36.0)]), num(1295.0));
        assert_eq!(call(1313, &[txt("1"), num(37.0)]), e(err::NUM));
        assert_eq!(call(1313, &[txt(&"1".repeat(256)), num(2.0)]), e(err::NUM));
        assert_eq!(call(1314, &[num(1.0)]), txt("000000000000f03f"));
        // Ten digits: negative when the first is at least half the base,
        // else positive (LibreOffice and Gnumeric -1, -1, 511).
        assert_eq!(call(1300, &[txt("1111111111")]), num(-1.0));
        assert_eq!(call(1307, &[txt("FFFFFFFFFF")]), num(-1.0));
        assert_eq!(call(1300, &[txt("0111111111")]), num(511.0));
    }

    #[test]
    fn bessel_edges() {
        let e = CellValue::Err;
        assert_eq!(call(1381, &[num(1.0), num(-1.0)]), e(err::NUM));
        assert_eq!(call(1382, &[num(0.0), num(1.0)]), e(err::NUM));
        assert_eq!(call(1383, &[num(-1.0), num(1.0)]), e(err::NUM));
        assert_eq!(call(1380, &[num(0.0), num(2.0)]), num(0.0));
        assert_eq!(call(1380, &[num(1e4), num(1.0)]), e(err::NUM), "overflow");
        // Orders past 1e7 are #NUM!: each function walked every order up to
        // n, and BESSELJ(1E9, 2E9) took 12 s. At the cap: Gnumeric's
        // BESSELY(1E9, 1E7)-sized calls finish in milliseconds.
        let t0 = std::time::Instant::now();
        for id in 1380..=1383 {
            assert_eq!(
                call(id, &[num(1e3), num(2_147_483_647.0)]),
                e(err::NUM),
                "{id}"
            );
            assert_eq!(
                call(id, &[num(1e9), num(1.000_000_1e7)]),
                e(err::NUM),
                "{id}"
            );
            let _ = call(id, &[num(1e9), num(1e7)]);
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
        // Gnumeric -7.131290907019298E-05 and -0.00024268737815770185.
        let near = |v: CellValue, want: f64| {
            let got = v.as_num().unwrap();
            assert!((got - want).abs() <= 1e-12 * want.abs(), "{got} vs {want}");
        };
        near(call(1381, &[num(1e7), num(1e6)]), -7.131_290_907_019_298e-5);
        near(
            call(1383, &[num(1e7), num(1e6)]),
            -2.426_873_781_577_018_5e-4,
        );
        // A NaN x runs the series to its 2000-term cap: #NUM!.
        assert_eq!(call(1380, &[num(f64::NAN), num(0.0)]), e(err::NUM));
        // I₂₀(5) (mpmath 5.024239357971806e-11; LibreOffice and Gnumeric).
        let i20 = call(1380, &[num(5.0), num(20.0)]).as_num().unwrap();
        assert!(
            (i20 / 5.024_239_357_971_806e-11 - 1.0).abs() < 1e-14,
            "{i20}"
        );
    }

    #[test]
    fn bit_edges() {
        let e = CellValue::Err;
        assert_eq!(call(1315, &[num(-1.0), num(1.0)]), e(err::NUM));
        assert_eq!(call(1316, &[num(1.0), num(BIT_LIMIT)]), e(err::NUM));
        assert_eq!(call(1318, &[num(1.0), num(54.0)]), e(err::NUM));
        assert_eq!(call(1318, &[num(1.0), num(53.0)]), num(2f64.powi(53)));
        assert_eq!(call(1319, &[num(1.0), num(-2.0)]), num(4.0));
        assert_eq!(call(1320, &[num(0.0)]), num(1.0));
        assert_eq!(call(1321, &[num(-1.0)]), num(0.0));
    }
}
