//! Complex numbers as spreadsheet text, `"3+4i"`: Excel's parse and format
//! rules (suffix `i` or `j`, 15 significant digits), glibc's kernels (as
//! LibreOffice calls them) for Excel's functions, and Gnumeric's (GSL's)
//! for its own (`IMTANH`, `IMCOTH`, the inverse functions, Γ).

use super::super::{b, err, n, ok_num, ok_text, R};
use crate::CellValue;
use std::f64::consts::{FRAC_PI_2, LN_10, LOG2_E, PI};

/// A complex number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cx {
    re: f64,
    im: f64,
}

const fn cx(re: f64, im: f64) -> Cx {
    Cx { re, im }
}

const ONE: Cx = cx(1.0, 0.0);

impl Cx {
    fn real(self) -> bool {
        self.im == 0.0
    }
    fn add(self, o: Cx) -> Cx {
        cx(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: Cx) -> Cx {
        cx(self.re - o.re, self.im - o.im)
    }
    fn mul(self, o: Cx) -> Cx {
        cx(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    fn scale(self, s: f64) -> Cx {
        cx(self.re * s, self.im * s)
    }
    fn neg(self) -> Cx {
        cx(-self.re, -self.im)
    }
    /// Smith's division, as libgcc's `__divdc3` does it.
    fn div(self, o: Cx) -> Cx {
        let (a, b, c, d) = (self.re, self.im, o.re, o.im);
        if c.abs() < d.abs() {
            let r = c / d;
            let den = c * r + d;
            cx((a * r + b) / den, (b * r - a) / den)
        } else {
            let r = d / c;
            let den = d * r + c;
            cx((b * r + a) / den, (b - a * r) / den)
        }
    }
    fn recip(self) -> Cx {
        ONE.div(self)
    }
    /// GSL's 1/z (Gnumeric's inverse functions).
    fn inverse(self) -> Cx {
        let s = 1.0 / self.abs();
        cx((self.re * s) * s, -(self.im * s) * s)
    }
    /// z times y·i.
    fn mul_imag(self, y: f64) -> Cx {
        cx(-y * self.im, y * self.re)
    }
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
    fn exp(self) -> Cx {
        let e = self.re.exp();
        if self.im == 0.0 {
            return cx(e, self.im);
        }
        let (s, c) = self.im.sin_cos();
        cx(e * c, e * s)
    }
    fn ln(self) -> Cx {
        cx(self.abs().ln(), self.arg())
    }
    /// glibc's `csqrt` (the sign of a zero imaginary part picks the side of
    /// the cut: √(−4−0i) = −2i).
    fn sqrt(self) -> Cx {
        let (x, y) = (self.re, self.im);
        if y == 0.0 {
            return if x < 0.0 {
                cx(0.0, (-x).sqrt().copysign(y))
            } else {
                cx(x.sqrt(), 0f64.copysign(y))
            };
        }
        if x == 0.0 {
            let r = (0.5 * y.abs()).sqrt();
            return cx(r, r.copysign(y));
        }
        let d = x.hypot(y);
        let (r, s) = if x > 0.0 {
            let r = (0.5 * (d + x)).sqrt();
            (r, 0.5 * (y / r))
        } else {
            let s = (0.5 * (d - x)).sqrt();
            ((0.5 * (y / s)).abs(), s)
        };
        cx(r, s.copysign(y))
    }
    /// libstdc++'s `pow(complex, double)`: a positive real base stays real,
    /// anything else goes through the polar form.
    fn powr(self, y: f64) -> Cx {
        if self.im == 0.0 && self.re > 0.0 {
            return cx(self.re.powf(y), 0.0);
        }
        let l = self.ln();
        let rho = (y * l.re).exp();
        let theta = y * l.im;
        cx(rho * theta.cos(), rho * theta.sin())
    }
    /// z^w = exp(w ln z) (Γ's Lanczos form).
    fn pow(self, w: Cx) -> Cx {
        w.mul(self.ln()).exp()
    }
    fn sin(self) -> Cx {
        let (s, c) = self.re.sin_cos();
        cx(self.im.cosh() * s, self.im.sinh() * c)
    }
    fn cos(self) -> Cx {
        let (s, c) = self.re.sin_cos();
        cx(self.im.cosh() * c, -(self.im.sinh() * s))
    }
    /// glibc's `ctan`.
    fn tan(self) -> Cx {
        let (s, c) = self.re.sin_cos();
        let (sh, ch) = (self.im.sinh(), self.im.cosh());
        let den = if sh.abs() > c.abs() * f64::EPSILON {
            c * c + sh * sh
        } else {
            c * c
        };
        cx(s * c / den, sh * ch / den)
    }
    fn sinh(self) -> Cx {
        let (s, c) = self.im.sin_cos();
        cx(self.re.sinh() * c, self.re.cosh() * s)
    }
    fn cosh(self) -> Cx {
        let (s, c) = self.im.sin_cos();
        cx(self.re.cosh() * c, self.re.sinh() * s)
    }
    /// GSL's `tanh` (Gnumeric's IMTANH).
    fn tanh(self) -> Cx {
        let (r, i) = (self.re, self.im);
        let d = i.cos().powi(2) + r.sinh().powi(2);
        let im = 0.5 * (2.0 * i).sin() / d;
        if r.abs() < 1.0 {
            cx(r.sinh() * r.cosh() / d, im)
        } else {
            let f = 1.0 + (i.cos() / r.sinh()).powi(2);
            cx(1.0 / (r.tanh() * f), im)
        }
    }
}

// --- GSL's inverse functions (Gnumeric's plugins/fn-complex/gsl-complex.c) ---

fn arcsin(a: Cx) -> Cx {
    let (rr, ii) = (a.re, a.im);
    if ii == 0.0 {
        return if rr.abs() <= 1.0 {
            cx(rr.asin(), 0.0)
        } else if rr < 0.0 {
            cx(-FRAC_PI_2, (-rr).acosh())
        } else {
            cx(FRAC_PI_2, -rr.acosh())
        };
    }
    let (x, y) = (rr.abs(), ii.abs());
    let (real, imag) = arc_parts(
        x,
        y,
        |b| b.asin(),
        |x, y, d, gt| {
            if gt {
                (x / (y * d.sqrt())).atan()
            } else {
                (x / d.sqrt()).atan()
            }
        },
    );
    cx(
        if rr >= 0.0 { real } else { -real },
        if ii >= 0.0 { imag } else { -imag },
    )
}

/// The shared core of GSL's arcsin and arccos (Hull et al.).
fn arc_parts(
    x: f64,
    y: f64,
    small: impl Fn(f64) -> f64,
    large: impl Fn(f64, f64, f64, bool) -> f64,
) -> (f64, f64) {
    let r = (x + 1.0).hypot(y);
    let s = (x - 1.0).hypot(y);
    let a = 0.5 * (r + s);
    let b = x / a;
    let y2 = y * y;
    let real = if b <= 0.6417 {
        small(b)
    } else if x <= 1.0 {
        let d = 0.5 * (a + x) * (y2 / (r + x + 1.0) + (s + (1.0 - x)));
        large(x, y, d, false)
    } else {
        let apx = a + x;
        let d = 0.5 * (apx / (r + x + 1.0) + apx / (s + (x - 1.0)));
        large(x, y, d, true)
    };
    let imag = if a <= 1.5 {
        let am1 = if x < 1.0 {
            0.5 * (y2 / (r + (x + 1.0)) + y2 / (s + (1.0 - x)))
        } else {
            0.5 * (y2 / (r + (x + 1.0)) + (s + (x - 1.0)))
        };
        (am1 + (am1 * (a + 1.0)).sqrt()).ln_1p()
    } else {
        (a + (a * a - 1.0).sqrt()).ln()
    };
    (real, imag)
}

fn arccos(a: Cx) -> Cx {
    let (rr, ii) = (a.re, a.im);
    if ii == 0.0 {
        return if rr.abs() <= 1.0 {
            cx(rr.acos(), 0.0)
        } else if rr < 0.0 {
            cx(PI, -(-rr).acosh())
        } else {
            cx(0.0, rr.acosh())
        };
    }
    let (x, y) = (rr.abs(), ii.abs());
    let (real, imag) = arc_parts(
        x,
        y,
        |b| b.acos(),
        |x, y, d, gt| {
            if gt {
                ((y * d.sqrt()) / x).atan()
            } else {
                (d.sqrt() / x).atan()
            }
        },
    );
    cx(
        if rr >= 0.0 { real } else { PI - real },
        if ii >= 0.0 { -imag } else { imag },
    )
}

fn arctan(a: Cx) -> Cx {
    let (rr, ii) = (a.re, a.im);
    if ii == 0.0 {
        return cx(rr.atan(), 0.0);
    }
    let r = rr.hypot(ii);
    let u = 2.0 * ii / (1.0 + r * r);
    let imag = if u.abs() < 0.1 {
        0.25 * (u.ln_1p() - (-u).ln_1p())
    } else {
        0.5 * (rr.hypot(ii + 1.0) / rr.hypot(ii - 1.0)).ln()
    };
    if rr == 0.0 {
        let re = if ii > 1.0 {
            FRAC_PI_2
        } else if ii < -1.0 {
            -FRAC_PI_2
        } else {
            0.0
        };
        cx(re, imag)
    } else {
        cx(0.5 * (2.0 * rr).atan2((1.0 + r) * (1.0 - r)), imag)
    }
}

fn arccot(a: Cx) -> Cx {
    if a.re == 0.0 && a.im == 0.0 {
        cx(FRAC_PI_2, 0.0)
    } else {
        arctan(a.inverse())
    }
}

fn arcsinh(a: Cx) -> Cx {
    arcsin(a.mul_imag(1.0)).mul_imag(-1.0)
}

fn arccosh(a: Cx) -> Cx {
    if a.im == 0.0 && a.re == 1.0 {
        return cx(0.0, 0.0);
    }
    let r = arccos(a);
    r.mul_imag(if r.im > 0.0 { -1.0 } else { 1.0 })
}

fn arctanh(a: Cx) -> Cx {
    if a.im == 0.0 {
        let x = a.re;
        return if x > -1.0 && x < 1.0 {
            cx(x.atanh(), 0.0)
        } else {
            // acoth(x) = atanh(1/x)
            cx(
                (1.0 / x).atanh(),
                if x < 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 },
            )
        };
    }
    arctan(a.mul_imag(1.0)).mul_imag(-1.0)
}

// --- Γ (Gnumeric's src/sf-gamma.c) -----------------------------------------

const LANCZOS_G: f64 = 808_618_867.0 / 134_217_728.0;
#[allow(
    clippy::excessive_precision,
    reason = "Gnumeric's coefficients, verbatim"
)]
const LANCZOS_NUM: [f64; 13] = [
    56906521.913471563880907910335591226868592353221448,
    103794043.11634454519062710536160702385539539810110,
    86363131.288138591455469272889778684223419113014358,
    43338889.324676138347737237405905333160850993321475,
    14605578.087685068084141699827913592185707234229516,
    3481712.1549806459088207101896477455646802362321652,
    601859.61716810987866702265336993523025071425740828,
    75999.293040145426498753034435989091370919973262979,
    6955.9996025153761403563101155151989875259157712039,
    449.94455690631681194468586076509884096232715968614,
    19.519927882476174828478609662356521362076846583112,
    0.50984166556566761881251786448046945099926051133936,
    0.0060618423462489065257837539645559368832224636654970,
];
const LANCZOS_DENOM: [f64; 13] = [
    0.0,
    39916800.0,
    120543840.0,
    150917976.0,
    105258076.0,
    45995730.0,
    13339535.0,
    2637558.0,
    357423.0,
    32670.0,
    1925.0,
    66.0,
    1.0,
];

fn gamma(z: Cx) -> Cx {
    if z.real() {
        return cx(libm::tgamma(z.re), 0.0);
    }
    if z.re < 0.0 {
        // Γ(z) = π / (sin(πz) Γ(1 − z)) = π / (sin(πz) · (−z) Γ(−z))
        let b = cx(PI * (z.re % 2.0), PI * z.im);
        return cx(PI, 0.0).div(fact(z.neg()).mul(b.sin()));
    }
    let mut p = cx(LANCZOS_NUM[12], 0.0);
    let mut q = cx(LANCZOS_DENOM[12], 0.0);
    for i in (0..12).rev() {
        p = p.mul(z);
        p.re += LANCZOS_NUM[i];
        q = q.mul(z);
        q.re += LANCZOS_DENOM[i];
    }
    let zmh = cx(z.re - 0.5, z.im);
    let f = zmh.add(cx(LANCZOS_G, 0.0)).pow(zmh.scale(0.5));
    f.mul(zmh.neg().exp()).mul(f).mul(p.div(q))
}

fn fact(z: Cx) -> Cx {
    if z.real() {
        cx(libm::tgamma(z.re + 1.0), 0.0)
    } else {
        gamma(z).mul(z)
    }
}

/// Gnumeric's complex incomplete Γ(a, z): lower or upper, regularized or not.
fn igamma(a: Cx, z: Cx, lower: bool, reg: bool) -> Cx {
    let (mut res, have_lower, mut have_reg);
    if reg && a.real() && a.re <= 0.0 && a.re == a.re.floor() {
        (res, have_lower, have_reg) = (cx(0.0, 0.0), false, true);
    } else if a.real() && a.re >= 0.0 && z.real() && z.re >= 0.0 {
        let p = real_igamma(a.re, z.re, lower);
        (res, have_lower, have_reg) = (cx(p, 0.0), lower, true);
    } else if let Some(r) = igamma_upper_asymp(a, z) {
        (res, have_lower, have_reg) = (fixup_upper_real(r, a, z), false, true);
    } else if let Some(r) = igamma_lower_cf(a, z) {
        (res, have_lower, have_reg) = (r, true, false);
    } else {
        return cx(f64::NAN, f64::NAN);
    }
    let mut ga = None;
    if reg != have_reg {
        let g = gamma(a);
        ga = Some(g);
        res = if have_reg { res.mul(g) } else { res.div(g) };
        have_reg = true;
    }
    if lower != have_lower {
        res = if have_reg {
            ONE.sub(res)
        } else {
            ga.unwrap_or_else(|| gamma(a)).sub(res)
        };
    }
    res
}

/// The regularized P(a, x) or Q(a, x) for real a, x ≥ 0 (Γ(0, x) is the
/// limit: P = 1).
fn real_igamma(a: f64, x: f64, lower: bool) -> f64 {
    if a == 0.0 {
        return f64::from(u8::from(lower));
    }
    let q = super::super::statistical::gamma_q(a, x);
    if lower {
        statrs::function::gamma::checked_gamma_lr(a, x).unwrap_or(1.0 - q)
    } else {
        q
    }
}

fn igamma_upper_asymp(a: Cx, z: Cx) -> Option<Cx> {
    let (am, zm) = (a.abs(), z.abs());
    if am >= zm || 2.0 * zm < 53.0 * std::f64::consts::LN_2 {
        return None;
    }
    let mut s = cx(0.0, 0.0);
    // D(a − 1, z) = z^(a−1) e^−z / Γ(a)
    let am1 = a.sub(ONE);
    let mut t = z.pow(am1).div(z.exp()).div(fact(am1));
    for i in 0..100 {
        s = s.add(t);
        if t.abs() <= s.abs() * f64::EPSILON {
            return Some(s);
        }
        t = t.div(z).mul(a.sub(cx(f64::from(i + 1), 0.0)));
    }
    None
}

fn fixup_upper_real(res: Cx, a: Cx, z: Cx) -> Cx {
    if z.real() && z.re < 0.0 && a.real() && a.re != a.re.floor() {
        let m = ONE.sub(res).abs();
        let (s, c) = sin_cos_pi(a.re);
        return ONE.sub(cx(m * c, m * s));
    }
    res
}

/// (sin πx, cos πx), exact at multiples of ½ (Gnumeric's sinpi / cospi).
fn sin_cos_pi(x: f64) -> (f64, f64) {
    let h = 2.0 * (x % 2.0);
    if h.fract() == 0.0 {
        return match h.rem_euclid(4.0) as i32 {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (-1.0, 0.0),
        };
    }
    (PI * x).sin_cos()
}

fn igamma_lower_cf(a: Cx, z: Cx) -> Option<Cx> {
    let (mut a0, mut b1) = (ONE, ONE);
    let (mut a1, mut b0) = (cx(0.0, 0.0), cx(0.0, 0.0));
    const BIG: f64 = 18_446_744_073_709_551_616.0;
    let mut converged = false;
    for i in 1..100usize {
        let ai = if i == 1 {
            ONE
        } else if i & 1 == 1 {
            z.scale((i >> 1) as f64)
        } else {
            cx(-(a.re + ((i >> 1) as f64 - 1.0)), -a.im).mul(z)
        };
        let bi = cx(a.re + (i as f64 - 1.0), a.im);
        let a2 = bi.mul(a1).add(ai.mul(a0));
        (a0, a1) = (a1, a2);
        let b2 = bi.mul(b1).add(ai.mul(b0));
        (b0, b1) = (b1, b2);
        let m = b1.re.abs() + b1.im.abs();
        if m >= BIG || m <= 1.0 / BIG {
            if m == 0.0 {
                return None;
            }
            let s = 2f64.powi(-libm::ilogb(m));
            (a0, a1, b0, b1) = (a0.scale(s), a1.scale(s), b0.scale(s), b1.scale(s));
        }
        let c1 = a1.mul(b0).sub(a0.mul(b1));
        let c2 = b0.mul(b1);
        if c1.abs() <= c2.abs() * (f64::EPSILON / 2.0) {
            converged = true;
            break;
        }
    }
    if !converged {
        return None;
    }
    Some(a1.div(b1).mul(z.neg().exp()).mul(z.pow(a)))
}

// --- text -------------------------------------------------------------------

/// A complex argument and its imaginary unit, if it names one: a number is
/// real; text must parse (else `#NUM!`); a boolean is `#VALUE!`.
fn arg(a: &[CellValue], i: usize) -> Result<(Cx, Option<char>), u8> {
    match a.get(i) {
        Some(CellValue::Num(_)) => Ok((cx(n(a, i)?, 0.0), None)),
        Some(CellValue::Text(s)) => parse(s).ok_or(err::NUM),
        _ => Err(err::VALUE),
    }
}

/// Excel's complex text: `a`, `bi`, `a+bi`, `a-bi`, `i`, `-i`, `a+i`, with
/// suffix `i` or `j`; no spaces, no upper case.
pub(super) fn parse(s: &str) -> Option<(Cx, Option<char>)> {
    let unit = |s: &str| match s {
        "i" => Some('i'),
        "j" => Some('j'),
        _ => None,
    };
    // A bare unit, with or without a sign.
    let signed_unit = |s: &str| -> Option<(f64, char)> {
        let (sign, rest) = match s.as_bytes().first()? {
            b'+' => (1.0, &s[1..]),
            b'-' => (-1.0, &s[1..]),
            _ => (1.0, s),
        };
        Some((sign, unit(rest)?))
    };
    if let Some((sign, u)) = signed_unit(s) {
        return Some((cx(0.0, sign), Some(u)));
    }
    let (x, rest) = number(s)?;
    if rest.is_empty() {
        return Some((cx(x, 0.0), None));
    }
    if let Some(u) = unit(rest) {
        return Some((cx(0.0, x), Some(u)));
    }
    if !rest.starts_with(['+', '-']) {
        return None;
    }
    if let Some((sign, u)) = signed_unit(rest) {
        return Some((cx(x, sign), Some(u)));
    }
    let (y, tail) = number(rest)?;
    Some((cx(x, y), Some(unit(tail)?)))
}

/// A leading decimal number (optional sign, digits with an optional point,
/// optional exponent) and the text after it.
fn number(s: &str) -> Option<(f64, &str)> {
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if !b[start..i].iter().any(u8::is_ascii_digit) {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    Some((s[..i].parse().ok()?, &s[i..]))
}

/// `x` as C's `%.15g` (Excel's and LibreOffice's 15 significant digits).
fn g15(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let e = format!("{x:.14e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let trim = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if !(-4..15).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mant), exp.abs())
    } else {
        trim(&format!("{x:.*}", (14 - exp) as usize))
    }
}

/// `z` as text with imaginary unit `u` (`#NUM!` if not finite).
fn show(z: Cx, u: Option<char>) -> R {
    if !(z.re.is_finite() && z.im.is_finite()) {
        return Err(err::NUM);
    }
    let u = u.unwrap_or('i');
    let has_real = z.im == 0.0 || z.re != 0.0;
    let mut s = if has_real { g15(z.re) } else { String::new() };
    if z.im != 0.0 {
        if z.im == 1.0 {
            if has_real {
                s.push('+');
            }
        } else if z.im == -1.0 {
            s.push('-');
        } else {
            let im = g15(z.im);
            if has_real && !im.starts_with('-') {
                s.push('+');
            }
            s.push_str(&im);
        }
        s.push(u);
    }
    ok_text(s)
}

/// The unit of a multi-argument result: all named units must agree (Excel:
/// "all suffixes must match", else `#VALUE!`).
fn join(u: Option<char>, v: Option<char>) -> Result<Option<char>, u8> {
    match (u, v) {
        (Some(x), Some(y)) if x != y => Err(err::VALUE),
        _ => Ok(u.or(v)),
    }
}

/// |x| ≤ 2⁶⁴, LibreOffice's limit for a trigonometric argument.
fn trig_ok(x: f64) -> Result<(), u8> {
    if x.abs() <= 18_446_744_073_709_551_616.0 {
        Ok(())
    } else {
        Err(err::NUM)
    }
}

/// Evaluate complex function `id`.
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    if id == 1330 {
        let suffix = if a.len() > 2 {
            match super::super::t(a, 2)?.as_str() {
                "i" | "" => 'i',
                "j" => 'j',
                _ => return Err(err::VALUE),
            }
        } else {
            'i'
        };
        return show(cx(n(a, 0)?, n(a, 1)?), Some(suffix));
    }
    if matches!(id, 1338 | 1339) {
        let (mut z, mut u) = arg(a, 0)?;
        for i in 1..a.len() {
            let (w, v) = arg(a, i)?;
            u = join(u, v)?;
            z = if id == 1338 { z.add(w) } else { z.mul(w) };
        }
        return show(z, u);
    }
    let (z, u) = arg(a, 0)?;
    let out = |w: Cx| show(w, u);
    match id {
        1331 => ok_num(z.re),
        1332 => ok_num(z.im),
        1333 => ok_num(z.abs()),
        1334 => {
            if z.re == 0.0 && z.im == 0.0 {
                return Err(err::NUM);
            }
            ok_num(z.arg())
        }
        1335 => out(cx(z.re, -z.im)),
        1336 => out(z.neg()),
        1337 => out(z.recip()),
        1340 | 1341 => {
            let (w, v) = arg(a, 1)?;
            let u = join(u, v)?;
            if id == 1340 {
                return show(z.sub(w), u);
            }
            if w.re == 0.0 && w.im == 0.0 {
                return Err(err::NUM);
            }
            show(z.div(w), u)
        }
        1342 => {
            let y = n(a, 1)?;
            if z.re == 0.0 && z.im == 0.0 && y <= 0.0 {
                return Err(err::NUM);
            }
            out(z.powr(y))
        }
        1343 => out(z.sqrt()),
        1344 => out(z.exp()),
        1345 => out(z.ln()),
        1346 => {
            let l = z.ln();
            out(cx(l.re / LN_10, l.im / LN_10))
        }
        1347 => out(z.ln().scale(LOG2_E)),
        1348 => {
            trig_ok(z.re)?;
            out(z.sin())
        }
        1349 => {
            trig_ok(z.re)?;
            out(z.cos())
        }
        1350 | 1353 => {
            trig_ok(if z.im != 0.0 { 2.0 * z.re } else { z.re })?;
            let w = z.tan();
            out(if id == 1350 { w } else { w.recip() })
        }
        1351 => {
            trig_ok(z.re)?;
            out(z.cos().recip())
        }
        1352 => {
            trig_ok(z.re)?;
            out(z.sin().recip())
        }
        1354 | 1358 => {
            trig_ok(z.im)?;
            let w = z.sinh();
            out(if id == 1354 { w } else { w.recip() })
        }
        1355 | 1357 => {
            trig_ok(z.im)?;
            let w = z.cosh();
            out(if id == 1355 { w } else { w.recip() })
        }
        1356 => out(z.tanh()),
        1359 => out(z.tanh().inverse()),
        1360 => out(arcsin(z)),
        1361 => out(arccos(z)),
        1362 => out(arctan(z)),
        1363 => out(arccos(z.inverse())),
        1364 => out(arcsin(z.inverse())),
        1365 => out(arccot(z)),
        1366 => out(arcsinh(z)),
        1367 => out(arccosh(z)),
        1368 => out(arctanh(z)),
        1369 => out(arccosh(z.inverse())),
        1370 => out(arcsinh(z.inverse())),
        1371 => out(arctanh(z.inverse())),
        1372 => out(fact(z)),
        1373 => out(gamma(z)),
        1374 => {
            let (w, v) = arg(a, 1)?;
            let flag = |i: usize| if a.len() > i { b(a, i) } else { Ok(true) };
            show(igamma(z, w, flag(2)?, flag(3)?), join(u, v)?)
        }
        _ => Err(err::NAME),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn parses_excel_forms() {
        let p = |s: &str| parse(s).map(|(z, u)| (z.re, z.im, u));
        assert_eq!(p("3+4i"), Some((3.0, 4.0, Some('i'))));
        assert_eq!(p("-i"), Some((0.0, -1.0, Some('i'))));
        assert_eq!(p("+j"), Some((0.0, 1.0, Some('j'))));
        assert_eq!(p("2j"), Some((0.0, 2.0, Some('j'))));
        assert_eq!(p("0"), Some((0.0, 0.0, None)));
        assert_eq!(p("1.5e2-2.5E-1i"), Some((150.0, -0.25, Some('i'))));
        assert_eq!(p("3-i"), Some((3.0, -1.0, Some('i'))));
        assert_eq!(p("1e"), None);
        for bad in [
            "", "i5", "3+", "3+-4i", "3 +4i", "1+2I", ".", "+", "3x", "3+4", "1,5",
        ] {
            assert_eq!(p(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn formats_like_printf_g15() {
        assert_eq!(g15(1e20), "1e+20");
        assert_eq!(g15(1e-5), "1e-05");
        assert_eq!(g15(0.0001), "0.0001");
        assert_eq!(g15(123456789012345.0), "123456789012345");
        assert_eq!(g15(1234567890123456.0), "1.23456789012346e+15");
        assert_eq!(g15(-2.5), "-2.5");
        assert_eq!(g15(1.0 / 3.0), "0.333333333333333");
        assert_eq!(g15(99999.99999999999), "100000");
    }

    #[test]
    fn kernels_cover_their_branches() {
        let close = |a: Cx, b: Cx| (a.re - b.re).abs() < 1e-12 && (a.im - b.im).abs() < 1e-12;
        // sqrt: positive real, negative real with a signed zero, pure imaginary
        assert_eq!(cx(4.0, 0.0).sqrt(), cx(2.0, 0.0));
        assert_eq!(cx(-4.0, -0.0).sqrt(), cx(0.0, -2.0));
        assert!(close(cx(0.0, 2.0).sqrt(), cx(1.0, 1.0)));
        assert!(close(cx(-3.0, 4.0).sqrt(), cx(1.0, 2.0)));
        // division: |c| < |d| and the other branch
        assert!(close(cx(1.0, 0.0).div(cx(0.0, 2.0)), cx(0.0, -0.5)));
        assert!(close(cx(1.0, 1.0).div(cx(2.0, 0.0)), cx(0.5, 0.5)));
        // real-axis inverse functions outside [-1, 1]
        assert!(close(arcsin(cx(-2.0, 0.0)), cx(-FRAC_PI_2, 2f64.acosh())));
        assert!(close(arccos(cx(-2.0, 0.0)), cx(PI, -(2f64.acosh()))));
        assert!(close(arccos(cx(2.0, 0.0)), cx(0.0, 2f64.acosh())));
        assert!(close(arctanh(cx(2.0, 0.0)), cx(0.5f64.atanh(), -FRAC_PI_2)));
        assert!(close(
            arctanh(cx(-2.0, 0.0)),
            cx((-0.5f64).atanh(), FRAC_PI_2)
        ));
        assert_eq!(arccot(cx(0.0, 0.0)), cx(FRAC_PI_2, 0.0));
        assert_eq!(arccosh(cx(1.0, 0.0)), cx(0.0, 0.0));
        // arctan on the imaginary axis
        assert!(arctan(cx(0.0, 2.0)).re == FRAC_PI_2);
        assert!(arctan(cx(0.0, -2.0)).re == -FRAC_PI_2);
        assert!(arctan(cx(0.0, 0.5)).re == 0.0);
        // tanh with |re| >= 1
        let t = cx(2.0, 1.0).tanh();
        assert!(close(
            t,
            cx(1.014_793_616_146_633_6, 0.033_812_826_079_896_69)
        ));
        // igamma's fallbacks
        assert_eq!(real_igamma(0.0, 1.0, false), 0.0);
        assert_eq!(real_igamma(2.0, 0.0, true), 0.0);
        assert_eq!(real_igamma(2.0, 0.0, false), 1.0);
        assert_eq!(real_igamma(0.0, 1.0, true), 1.0);
        let up = igamma(cx(1.0, 1.0), cx(40.0, 0.0), false, true);
        assert!(up.abs() < 1e-15 && up.abs() > 0.0);
        let un = igamma(cx(1.0, 1.0), cx(2.0, 0.0), true, false);
        assert!(close(
            un,
            igamma(cx(1.0, 1.0), cx(2.0, 0.0), true, true).mul(gamma(cx(1.0, 1.0)))
        ));
        let fx = fixup_upper_real(cx(0.5, 0.0), cx(0.5, 0.0), cx(-1.0, 0.0));
        assert!(close(fx, ONE.sub(cx(0.0, 0.5))));
    }

    #[test]
    fn suffixes_must_agree() {
        let t = |s: &str| CellValue::Text(s.into());
        assert_eq!(eval(1338, &[t("1+i"), t("2+j")]), Err(err::VALUE));
        assert_eq!(eval(1340, &[t("1+i"), t("2+j")]), Err(err::VALUE));
        assert_eq!(eval(1331, &[CellValue::Bool(true)]), Err(err::VALUE));
        assert_eq!(
            eval(1330, &[CellValue::num(1.0), CellValue::num(1.0), t("k")]),
            Err(err::VALUE)
        );
    }
}
