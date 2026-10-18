//! Gnumeric's Fourier tools (plugins/fn-tsa): its radix-2 FFT, ported
//! as written (each level halves, so the transform is scaled by 1/n), and
//! the transform's text form `a+bi` (`gnm_complex_to_string`).

use super::List;
use crate::builtins::{err, n_or};
use crate::reduce::statistical::number;
use crate::reduce::Rows;
use crate::CellValue;

type C = (f64, f64);

/// `gnm_fourier_fft` of `input[0], input[skip], …` (n a power of two).
fn fft(input: &[C], n: usize, skip: usize, inverse: bool) -> Vec<C> {
    if n == 1 {
        return vec![input[0]];
    }
    let half = n / 2;
    let f1 = fft(input, half, skip * 2, inverse);
    let f2 = fft(&input[skip..], half, skip * 2, inverse);
    let step = if inverse { 1.0 } else { -1.0 } * std::f64::consts::PI / half as f64;
    let mut out = vec![(0.0, 0.0); n];
    for i in 0..half {
        let (c, s) = ((step * i as f64).cos(), (step * i as f64).sin());
        let t = (f2[i].0 * c - f2[i].1 * s, f2[i].0 * s + f2[i].1 * c);
        out[i] = ((f1[i].0 + t.0) * 0.5, (f1[i].1 + t.1) * 0.5);
        out[i + half] = ((f1[i].0 - t.0) * 0.5, (f1[i].1 - t.1) * 0.5);
    }
    out
}

/// The numbers of the group (non-numbers skipped), zero-padded to a power
/// of two, transformed. None is #VALUE!.
fn transform(ys: &[f64], inverse: bool) -> Result<Vec<C>, u8> {
    if ys.is_empty() {
        return Err(err::VALUE);
    }
    let nb = ys.len().next_power_of_two();
    let mut input: Vec<C> = ys.iter().map(|y| (*y, 0.0)).collect();
    input.resize(nb, (0.0, 0.0));
    Ok(fft(&input, nb, 1, inverse))
}

fn numbers(rows: &Rows) -> Vec<f64> {
    rows.col(0).filter_map(number).collect()
}

/// A number the way Gnumeric's `go_dtoa("!^G")` writes it: the shortest
/// round-trip digits, in exponent form (`1.5E-07`, `2E+20`) below 1e-4 or
/// from 1e17.
fn shortest(x: f64) -> String {
    let sci = format!("{x:e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    if x != 0.0 && !(-4..17).contains(&e) {
        let sign = if e < 0 { '-' } else { '+' };
        format!("{mant}E{sign}{:02}", e.abs())
    } else {
        format!("{x}")
    }
}

/// `gnm_complex_to_string(z, 'i')`.
fn complex_text((re, im): C) -> String {
    let real = re != 0.0 || im == 0.0;
    let mut s = if real { shortest(re) } else { String::new() };
    if im != 0.0 {
        if im == 1.0 {
            if real {
                s.push('+');
            }
        } else if im == -1.0 {
            s.push('-');
        } else {
            let t = shortest(im);
            if real && !t.starts_with(['-', '+']) {
                s.push('+');
            }
            s.push_str(&t);
        }
        s.push('i');
    }
    s
}

/// FOURIER(y OVER T, [inverse = FALSE] INTO K): Gnumeric's discrete
/// Fourier transform (scaled by 1/n, zero-padded to a power of two), each
/// value as complex text "a+bi".
pub(super) fn fourier(rows: &Rows, a: &[CellValue]) -> List {
    let inverse = n_or(a, 0, 0.0)?.floor() != 0.0;
    Ok(transform(&numbers(rows), inverse)?
        .into_iter()
        .map(|z| CellValue::Text(complex_text(z)))
        .collect())
}

/// PERIODOGRAM(y OVER T, [filter = 0] INTO K): the magnitudes of the
/// first half of the transform (Gnumeric), after a window: 0 none, 1
/// Bartlett, 2 Hann, 3 Welch. Fewer than 2 values, or another filter, is
/// #VALUE!.
pub(super) fn periodogram(rows: &Rows, a: &[CellValue]) -> List {
    let filter = n_or(a, 0, 0.0)?.floor();
    if !(0.0..=3.0).contains(&filter) {
        return Err(err::VALUE);
    }
    let mut ys = numbers(rows);
    let len = ys.len();
    // Gnumeric's `n0 / 2` is integer division.
    let half = (len / 2) as f64;
    for (i, y) in ys.iter_mut().enumerate() {
        let i = i as f64;
        *y *= match filter as u8 {
            1 => 1.0 - (i / half - 1.0).abs(),
            2 => 0.5 * (1.0 - (2.0 * std::f64::consts::PI / len as f64 * i).cos()),
            3 => 1.0 - (i / half - 1.0) * (i / half - 1.0),
            _ => 1.0,
        };
    }
    let out = transform(&ys, false)?;
    let keep = out.len() / 2;
    if keep == 0 {
        return Err(err::VALUE);
    }
    Ok(out[..keep]
        .iter()
        .map(|(re, im)| CellValue::num((re * re + im * im).sqrt()))
        .collect())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn complex_text_forms() {
        assert_eq!(complex_text((0.0, 0.0)), "0");
        assert_eq!(complex_text((0.0, 1.0)), "i");
        assert_eq!(complex_text((2.0, 1.0)), "2+i");
        assert_eq!(complex_text((0.0, -1.0)), "-i");
        assert_eq!(complex_text((-0.5, 0.5)), "-0.5+0.5i");
        assert_eq!(complex_text((0.0, -2.5)), "-2.5i");
        assert_eq!(complex_text((1.5e-7, 0.0)), "1.5E-07");
        assert_eq!(complex_text((2e20, -3e-17)), "2E+20-3E-17i");
        assert_eq!(complex_text((1e100, 0.0)), "1E+100");
    }

    #[test]
    fn too_few_and_bad_filters() {
        let one = Rows {
            pos: vec![vec![0]],
            vals: vec![vec![CellValue::num(3.0)]],
            capacity: 1,
        };
        assert_eq!(periodogram(&one, &[]), Err(err::VALUE));
        assert_eq!(periodogram(&one, &[CellValue::num(4.0)]), Err(err::VALUE));
        let none = Rows {
            pos: vec![vec![0]],
            vals: vec![vec![CellValue::Text("x".into())]],
            capacity: 1,
        };
        assert_eq!(fourier(&none, &[]), Err(err::VALUE));
        assert_eq!(fourier(&one, &[]), Ok(vec![CellValue::Text("3".into())]));
    }
}
