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
/// Left of the imaginary axis (Re z ≤ 0), where Gnumeric gives #NUM!, it
/// is mpmath's principal branch (see [`igamma_left`]). At a pole of Γ(a)
/// (a = 0, −1, −2, …) P is 1 and Q is 0, the lower γ(a, z) is a pole
/// (#NUM!) and the upper Γ(a, z) is finite.
fn igamma(a: Cx, z: Cx, lower: bool, reg: bool) -> Cx {
    let (mut res, have_lower, mut have_reg);
    let pole = is_pole(a);
    if reg && pole {
        (res, have_lower, have_reg) = (cx(0.0, 0.0), false, true);
    } else if a.real() && a.re > 0.0 && z.real() && z.re >= 0.0 {
        let p = real_igamma(a.re, z.re, lower);
        (res, have_lower, have_reg) = (cx(p, 0.0), lower, true);
    } else if let Some(r) = upper_asymp(a, z) {
        // Q = Γ(a, z) / Γ(a), except at a pole, where Γ(a) is infinite.
        (res, have_lower, have_reg) = if pole {
            (r, false, false)
        } else {
            (fixup_upper_real(r.div(fact(a.sub(ONE))), a, z), false, true)
        };
    } else if let Some(r) = igamma_upper_cf(a, z) {
        (res, have_lower, have_reg) = (r, false, false);
    } else if let Some((r, low)) = igamma_left(a, z) {
        (res, have_lower, have_reg) = (r, low, false);
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
        have_reg = reg;
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

/// Whether Γ(a) has a pole at a: a = 0, −1, −2, …
fn is_pole(a: Cx) -> bool {
    a.real() && a.re <= 0.0 && a.re == a.re.floor()
}

/// The regularized P(a, x) or Q(a, x) for real a > 0, x ≥ 0, through the
/// statistics kernels: they turn a shape past 1e12 into NaN (#NUM!), where
/// statrs's continued fraction runs √a terms and its i32 term counter
/// overflows (a panic, or in release a hang).
fn real_igamma(a: f64, x: f64, lower: bool) -> f64 {
    use super::super::statistical::{gamma_p, gamma_q};
    if lower {
        gamma_p(a, x)
    } else {
        gamma_q(a, x)
    }
}

/// The asymptotic Γ(a, z) ~ z^(a−1) e^−z Σ (a−1)(a−2)…(a−k) / z^k, for
/// |z| > |a| and |z| ≥ 18.4 (Gnumeric's D(a − 1, z) series, unregularized
/// so that it also serves a pole of Γ(a)).
fn upper_asymp(a: Cx, z: Cx) -> Option<Cx> {
    let (am, zm) = (a.abs(), z.abs());
    if am >= zm || 2.0 * zm < 53.0 * std::f64::consts::LN_2 {
        return None;
    }
    let mut s = cx(0.0, 0.0);
    let am1 = a.sub(ONE);
    let mut t = z.pow(am1).div(z.exp());
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

/// Γ(a, z) for Re z > 0 once |z| passes [`upper_cf_edge`]: Legendre's
/// continued fraction z^a e^−z / (z+1−a − 1(1−a)/(z+3−a − 2(2−a)/(z+5−a − …)))
/// by modified Lentz. There the lower series' 1 − P cancels (Gnumeric's
/// route: Q(½, 30 + i) came out 1.2e-5 for 5.0e-15), and this fraction
/// converges in tens to hundreds of terms. Inside the edge the fraction is
/// the worse of the two, so None hands back to the lower series.
// ponytail: against mpmath both routes stay within ~2e-13 of each other
// at the edge for |a| ≤ 30; past that a thin band (|z| ≈ |a|, z near the
// imaginary axis) loses up to ~1e-12 on the fraction's side. A three-way
// switch or a double-double Lentz would close it.
fn igamma_upper_cf(a: Cx, z: Cx) -> Option<Cx> {
    if z.re <= 0.0 || z.abs() <= upper_cf_edge(a) {
        return None;
    }
    legendre_cf(a, z)
}

/// Legendre's fraction itself. It converges anywhere off the cut (z not a
/// negative real), slowly next to it; None when 10 000 terms do not settle.
fn legendre_cf(a: Cx, z: Cx) -> Option<Cx> {
    let mut b = z.add(ONE).sub(a);
    let mut c = cx(1e300, 0.0);
    let mut d = b.recip();
    let mut h = d;
    for i in 1..10_000 {
        let i = f64::from(i);
        let an = cx(i - a.re, -a.im).scale(-i);
        b.re += 2.0;
        d = an.mul(d).add(b).recip();
        c = b.add(an.div(c));
        let step = d.mul(c);
        h = h.mul(step);
        if step.sub(ONE).abs() < f64::EPSILON {
            return Some(a.mul(z.ln()).sub(z).exp().mul(h));
        }
    }
    None
}

/// The |z| past which [`igamma_upper_cf`] beats 1 − P: Re a + |Im a|/2,
/// shrunk for Re a < 0 (where the lower series cancels sooner) by
/// |Im a|/|Re a|, and never under ¼.
fn upper_cf_edge(a: Cx) -> f64 {
    let half_im = a.im.abs() / 2.0;
    let edge = if a.re >= 0.0 {
        a.re + half_im
    } else {
        half_im * (a.im.abs() / -a.re).min(1.0)
    };
    edge.max(0.25)
}

/// γ(a, z) or Γ(a, z) (the bool: lower) on mpmath's principal branch for
/// Re z ≤ 0, z ≠ 0, where the upper fraction does not reach and Gnumeric
/// gives #NUM! (and for a pole of Γ(a), a = 0, −1, −2, …, anywhere). The
/// lower series γ(a, z) = z^a Σ (−z)^k / (k! (a + k)) converges for every z,
/// but its terms reach e^|z| while the sum is about e^−Re z: it loses
/// e^(|z| + Re z), nothing on the negative real axis, all of it near the
/// imaginary one. Past a loss of e⁴, Legendre's fraction gives Γ(a, z) once
/// |z| passes [`upper_cf_edge`] (inside it, the lower fraction does, which
/// stays accurate for a large Re a). igamma gets the other forms from
/// Γ(a, z) = Γ(a) − γ(a, z). At a pole the series has one too, and Γ(a, z)
/// is finite: E₁(z) = −γₑ − ln z − Σ (−z)^k / (k·k!), then
/// Γ(a, z) = (z^a e^−z − Γ(a + 1, z)) / −a up to a.
// shortcut: the upward recurrence loses about |z|^m near the negative real
// axis for a = −m (3e-14 at m = 3, z = −12), and for Re a < 0 with
// |Im a| > 8 a z inside the edge takes the series' loss; a Temme-style
// expansion would close both, if a model ever needs them.
fn igamma_left(a: Cx, z: Cx) -> Option<(Cx, bool)> {
    let pole = is_pole(a);
    if z.abs() == 0.0 || (z.re > 0.0 && !pole) {
        return None;
    }
    let far = z.abs() + z.re > 4.0;
    if far && z.abs() > upper_cf_edge(a) {
        return legendre_cf(a, z).map(|r| (r, false));
    }
    if far && a.re > 0.0 {
        return igamma_lower_cf(a, z).map(|r| (r, true));
    }
    if pole {
        // A loop bound: past a = −1000, z^a has long overflowed (#NUM!).
        let m = (-a.re).min(1001.0) as u32;
        let e1 = series(z, |k| cx(k, 0.0))
            .neg()
            .sub(z.ln())
            .sub(cx(EULER_GAMMA, 0.0));
        let w = z.neg().exp();
        let g = (1..=m).fold(e1, |g, j| {
            let j = f64::from(j);
            g.sub(z.powr(-j).mul(w)).scale(-1.0 / j)
        });
        return Some((g, false));
    }
    let s = series(z, |k| a.add(cx(k, 0.0))).add(a.recip());
    Some((z.pow(a).mul(s), true))
}

/// The Euler–Mascheroni constant γₑ.
const EULER_GAMMA: f64 = 0.577_215_664_901_532_9;

/// Σ_{k ≥ 1} (−z)^k / (k! · den(k)), until a term no longer moves the sum.
/// It ends: past k = |z| the terms shrink geometrically (to 0 at worst),
/// and an overflow (inf) or NaN stops it at once.
fn series(z: Cx, den: impl Fn(f64) -> Cx) -> Cx {
    let (mut s, mut t, mut k) = (cx(0.0, 0.0), ONE, 0.0);
    loop {
        k += 1.0;
        t = t.mul(z.neg()).scale(1.0 / k);
        let d = t.div(den(k));
        s = s.add(d);
        // Also stops on NaN (an overflowed sum).
        if d.abs().partial_cmp(&(s.abs() * f64::EPSILON)) != Some(std::cmp::Ordering::Greater) {
            return s;
        }
    }
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
        assert_eq!(real_igamma(2.0, 0.0, true), 0.0);
        assert_eq!(real_igamma(2.0, 0.0, false), 1.0);
        // A real shape past 1e12 is NaN (#NUM!), lower and upper alike:
        // IMIGAMMA("1e16", "1e16") overflowed statrs's term counter.
        let t0 = std::time::Instant::now();
        assert!(real_igamma(1e16, 1e16, true).is_nan());
        assert!(real_igamma(1e300, 1e300, false).is_nan());
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
        assert!((real_igamma(2.0, 3.0, true) - 0.800_851_726_528_544).abs() < 1e-15);
        let big = [
            CellValue::Text("1e16".into()),
            CellValue::Text("1e16".into()),
        ];
        assert_eq!(eval(1374, &big), Err(err::NUM));
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

    /// The arc and incomplete-gamma kernels off the real axis, against
    /// mpmath 1.3 (30 digits): asin/acos in each of GSL's regions (b ≤
    /// 0.6417 or not, x ≤ 1 or not, a ≤ 1.5 or not), and Γ(a, z) by each
    /// route (real, asymptotic upper, continued-fraction lower; the upper
    /// fraction has its own test below), each
    /// normalization converted both ways.
    #[test]
    fn kernels_match_mpmath() {
        let near = |got: Cx, want: (f64, f64)| {
            let err = (got.re - want.0).abs().max((got.im - want.1).abs());
            assert!(
                err <= 1e-13 * want.0.abs().max(want.1.abs()).max(1e-300),
                "{got:?} vs {want:?}"
            );
        };
        for (x, y, asin, acos) in [
            (
                0.5,
                0.2,
                (0.509_085_659_365_267_1, 0.227_088_265_409_072_9),
                (1.061_710_667_429_629_5, -0.227_088_265_409_072_9),
            ),
            (
                0.9,
                0.3,
                (0.933_412_402_833_298_3, 0.484_894_845_670_678_36),
                (0.637_383_923_961_598_3, -0.484_894_845_670_678_36),
            ),
            (
                1.5,
                0.3,
                (1.314_834_518_658_652_5, 1.006_300_297_140_896_3),
                (0.255_961_808_136_244, -1.006_300_297_140_896_3),
            ),
            (
                2.5,
                0.1,
                (1.527_188_309_471_959_8, 1.567_836_912_152_937_2),
                (0.043_608_017_322_936_88, -1.567_836_912_152_937_2),
            ),
            (
                0.95,
                0.01,
                (1.251_694_985_925_331_2, 0.031_870_833_234_680_23),
                (0.319_101_340_869_565_4, -0.031_870_833_234_680_23),
            ),
            (
                1.2,
                0.05,
                (1.496_031_521_378_164, 0.627_407_406_636_572_9),
                (0.074_764_805_416_732_44, -0.627_407_406_636_572_9),
            ),
        ] {
            near(arcsin(cx(x, y)), asin);
            near(arccos(cx(x, y)), acos);
        }
        // Γ(a, z): (a, z, lower, regularized) -> mpmath gammainc.
        for (a, z, lower, reg, want) in [
            (
                (2.5, 0.0),
                (-3.0, 0.0),
                false,
                true,
                (1.0, -47.507_308_716_401_2),
            ),
            (
                (0.5, 0.0),
                (-1.0, 0.0),
                false,
                true,
                (1.0, -1.650_425_758_797_542_8),
            ),
            (
                (-1.5, 0.0),
                (2.0, 1.0),
                true,
                true,
                (1.001_535_470_625_329_2, 0.003_766_161_394_272_8),
            ),
            (
                (1.0, 1.0),
                (2.0, 0.0),
                true,
                false,
                (0.433_383_623_945_397_3, -0.267_790_122_351_236_6),
            ),
            (
                (2.0, 0.0),
                (1.0, 1.0),
                false,
                false,
                (0.707_092_096_345_938_1, -0.420_353_640_959_811_5),
            ),
            (
                (0.5, 0.5),
                (1.0, 0.5),
                true,
                true,
                (0.812_423_223_757_745_9, -0.077_981_000_515_356_88),
            ),
            (
                (3.0, 0.0),
                (30.0, 0.0),
                false,
                false,
                (9.002_033_296_024_248e-11, 0.0),
            ),
            (
                (2.0, 0.0),
                (1.0, 0.0),
                false,
                false,
                (0.735_758_882_342_884_7, 0.0),
            ),
            ((0.0, 0.0), (1.0, 1.0), true, true, (1.0, 0.0)),
            ((-2.0, 0.0), (1.0, 0.0), true, true, (1.0, 0.0)),
        ] {
            let got = igamma(cx(a.0, a.1), cx(z.0, z.1), lower, reg);
            // The asymptotic series is good to ~1e-14 relative of its 1.
            let err = (got.re - want.0).abs().max((got.im - want.1).abs());
            assert!(
                err <= 1e-12 * want.0.abs().max(want.1.abs()),
                "Γ({a:?}, {z:?}) = {got:?} vs {want:?}"
            );
        }
        // Large |z|: the asymptotic upper series (with the real-axis fixup
        // for a negative z and fractional a), and the lower continued
        // fraction's rescaling when its terms grow past 2⁶⁴.
        let rel = |got: Cx, want: (f64, f64)| {
            let err = (got.re - want.0).abs().max((got.im - want.1).abs());
            assert!(
                err <= 1e-12 * want.0.abs().max(want.1.abs()),
                "{got:?} vs {want:?}"
            );
        };
        rel(
            igamma(cx(2.5, 0.0), cx(-40.0, 0.0), false, true),
            (1.0, -4.313_680_801_634_244e19),
        );
        rel(
            igamma(cx(1.0, 1.0), cx(-40.0, 0.0), false, true),
            (-1.325_379_762_704_083_2e16, -1.429_786_396_467_333_4e16),
        );
        rel(
            igamma(cx(0.5, 0.0), cx(40.0, 1.0), false, true),
            (1.984_052_943_342_761_7e-19, -3.174_526_896_360_229_3e-19),
        );
        rel(
            igamma(cx(0.5, 0.0), cx(-40.0, 1.0), false, true),
            (-1.774_691_567_129_037e16, -1.171_945_157_672_269_6e16),
        );
        rel(
            igamma(cx(50.0, 0.0), cx(-60.0, 1.0), true, false),
            (-2.128_541_460_358_092_3e112, -8.220_649_392_093_982e112),
        );
        rel(
            igamma(cx(-3.5, 0.0), cx(1.0, 1.0), true, false),
            (0.284_861_210_976_558_87, -0.016_590_073_882_387_157),
        );
        // An integer a needs no fixup (Gnumeric -9218138712.786015; the
        // asymptotic series' tiny imaginary part is rounding).
        let int_a = igamma(cx(2.0, 0.0), cx(-20.0, 0.0), false, true);
        assert!(
            (int_a.re / -9_218_138_712.786_015 - 1.0).abs() < 1e-14,
            "{int_a:?}"
        );
        // Neither the asymptotic series (|a| near |z|) nor the upper
        // fraction (Re z < 0) applies and the lower fraction never settles:
        // NaN, shown as #NUM!.
        let nan = igamma(cx(-200.0, 1.0), cx(-150.0, 1.0), false, true);
        assert!(nan.re.is_nan() && nan.im.is_nan());
        assert!(igamma_lower_cf(cx(1.0, 0.0), cx(-500.0, 1.0)).is_none());
        // a = 0 zeroes the lower fraction's first denominator, and the
        // upper one covers Re z > 0 only, so Gnumeric gives Γ(0, −1) as
        // #NUM!. It is E₁(−1) = −Ei(1) − iπ on the principal branch.
        assert!(igamma_lower_cf(cx(0.0, 0.0), cx(-1.0, 0.0)).is_none());
        let e1 = igamma(cx(0.0, 0.0), cx(-1.0, 0.0), false, false);
        assert!(
            e1.sub(cx(-1.895_117_816_355_936_8, -PI)).abs() < 1e-15,
            "{e1:?}"
        );
        // Γ(0, 0) is infinite, and the lower γ at a pole of Γ(a) too: #NUM!.
        assert!(igamma(cx(0.0, 0.0), cx(0.0, 0.0), false, false).re.is_nan());
        assert!(!igamma(cx(-1.0, 0.0), cx(-1.0, 0.0), true, false)
            .re
            .is_finite());
        // P and Q at a pole: 1 and 0, for any z.
        assert_eq!(igamma(cx(0.0, 0.0), cx(-1.0, 0.0), true, true), ONE);
        assert_eq!(
            igamma(cx(-2.0, 0.0), cx(3.0, 0.0), false, true),
            cx(0.0, 0.0)
        );
        // sqrt with a positive and a negative real part; sinπ/cosπ at
        // halves and elsewhere.
        assert_eq!(cx(3.0, 4.0).sqrt(), cx(2.0, 1.0));
        assert_eq!(cx(-3.0, 4.0).sqrt(), cx(1.0, 2.0));
        assert_eq!(sin_cos_pi(0.5), (1.0, 0.0));
        assert_eq!(sin_cos_pi(1.0), (0.0, -1.0));
        let (s4, c4) = sin_cos_pi(0.25);
        assert!((s4 - FRAC_PI_2.sin() / 2f64.sqrt()).abs() < 1e-15 && (c4 - s4).abs() < 1e-15);
    }

    /// Re z ≤ 0 (and the poles of Γ(a)), where Gnumeric 1.12 gives #NUM!,
    /// against mpmath 1.3's gammainc on the principal branch (250 digits;
    /// scripts/goldens/imigamma_left.py): γ = Γ(a) − Γ(a, z) and the
    /// regularized forms over Γ(a), for every lower/regularize pair.
    #[test]
    #[allow(
        clippy::approx_constant,
        reason = "mpmath's digits, verbatim (some are ±π/2, π/6)"
    )]
    fn left_half_plane_matches_mpmath() {
        type Row = ((f64, f64), (f64, f64), bool, bool, (f64, f64));
        let rows: &[Row] = &[
            (
                (0.5, 0.0),
                (-1.0, 0.0),
                true,
                true,
                (0.0, 1.6504257587975428),
            ),
            (
                (0.5, 0.0),
                (-5.0, 0.0),
                false,
                true,
                (1.0, -43.32762975244078),
            ),
            (
                (0.5, 0.0),
                (-3.0, -4.0),
                true,
                true,
                (-0.536643565778565, 5.049143703447035),
            ),
            (
                (0.5, 0.0),
                (-10.0, 1.0),
                false,
                true,
                (-3361.258811653456, -2440.0975408702448),
            ),
            (
                (1.0, 0.0),
                (-1.0, 0.0),
                true,
                true,
                (-1.7182818284590453, 0.0),
            ),
            (
                (1.0, 0.0),
                (-5.0, 0.0),
                false,
                true,
                (148.4131591025766, 0.0),
            ),
            (
                (1.0, 0.0),
                (-3.0, -4.0),
                true,
                true,
                (14.128783081462158, 15.200784463067954),
            ),
            (
                (1.0, 0.0),
                (-10.0, 1.0),
                false,
                true,
                (11900.950259059775, -18534.631864193456),
            ),
            (
                (2.5, 0.0),
                (-1.0, 0.0),
                true,
                true,
                (0.0, 0.6280082302883814),
            ),
            (
                (2.5, 0.0),
                (-5.0, 0.0),
                false,
                true,
                (1.0, -917.081789416522),
            ),
            (
                (2.5, 0.0),
                (-3.0, -4.0),
                true,
                true,
                (-82.92536490812876, -113.45792708253342),
            ),
            (
                (2.5, 0.0),
                (-10.0, 1.0),
                false,
                true,
                (-416760.6579435658, -179103.31056880148),
            ),
            (
                (-0.5, 0.0),
                (-1.0, 0.0),
                true,
                true,
                (0.0, 0.11679946603380065),
            ),
            (
                (-0.5, 0.0),
                (-5.0, 0.0),
                false,
                true,
                (1.0, -5.881022909694438),
            ),
            (
                (-0.5, 0.0),
                (-3.0, -4.0),
                true,
                true,
                (1.4123816047046476, 0.3710697885959466),
            ),
            (
                (-0.5, 0.0),
                (-10.0, 1.0),
                false,
                true,
                (-172.266811579394, -160.40990072359517),
            ),
            (
                (-2.5, 0.0),
                (-1.0, 0.0),
                true,
                true,
                (0.0, -1.800233399920877),
            ),
            (
                (-2.5, 0.0),
                (-5.0, 0.0),
                false,
                true,
                (1.0, -1.0129640201374137),
            ),
            (
                (-2.5, 0.0),
                (-3.0, -4.0),
                true,
                true,
                (1.0039768615657831, -0.0823727639589431),
            ),
            (
                (-2.5, 0.0),
                (-10.0, 1.0),
                false,
                true,
                (-5.821945819831671, -10.484595208469404),
            ),
            ((0.0, 0.0), (-1.0, 0.0), true, true, (1.0, 0.0)),
            ((0.0, 0.0), (-12.0, 0.0), false, true, (0.0, 0.0)),
            (
                (0.0, 0.0),
                (0.0, 3.0),
                false,
                false,
                (-0.11962978600800032, 0.27785620120457166),
            ),
            ((-1.0, 0.0), (-1.0, 0.0), true, true, (1.0, 0.0)),
            ((-1.0, 0.0), (-12.0, 0.0), false, true, (0.0, 0.0)),
            (
                (-1.0, 0.0),
                (0.0, 3.0),
                false,
                false,
                (0.07258978332137792, 0.05214129766224351),
            ),
            ((-3.0, 0.0), (-1.0, 0.0), true, true, (1.0, 0.0)),
            ((-3.0, 0.0), (-12.0, 0.0), false, true, (0.0, 0.0)),
            (
                (-3.0, 0.0),
                (0.0, 3.0),
                false,
                false,
                (-0.004492674839533322, -0.006145246793209368),
            ),
            (
                (1.0, 1.0),
                (-1.0, 0.0),
                true,
                true,
                (-0.10567851915185654, 0.03321094056911651),
            ),
            (
                (1.0, 1.0),
                (-5.0, 0.0),
                false,
                true,
                (-0.2168694097510336, 11.579327040166657),
            ),
            (
                (1.0, 1.0),
                (-3.0, -4.0),
                true,
                true,
                (-264.7715639188388, 129.6121453555461),
            ),
            (
                (1.0, 1.0),
                (-10.0, 1.0),
                false,
                true,
                (150.96042341452645, 2019.1023530475532),
            ),
            (
                (-1.5, 2.0),
                (-1.0, 0.0),
                true,
                true,
                (-0.030491751711007466, -0.09436185422118547),
            ),
            (
                (-1.5, 2.0),
                (-5.0, 0.0),
                false,
                true,
                (1.2090664879236328, -0.21649270707830812),
            ),
            (
                (-1.5, 2.0),
                (-3.0, -4.0),
                true,
                true,
                (452.4193107395134, 1026.2644228420527),
            ),
            (
                (-1.5, 2.0),
                (-10.0, 1.0),
                false,
                true,
                (10.567527755105358, 3.264640662219112),
            ),
            (
                (3.0, -1.0),
                (-1.0, 0.0),
                true,
                true,
                (-3.3602974480502343, -9.084340841086266),
            ),
            (
                (3.0, -1.0),
                (-5.0, 0.0),
                false,
                true,
                (30385.42953296505, -17189.074509938324),
            ),
            (
                (3.0, -1.0),
                (-3.0, -4.0),
                true,
                true,
                (-23.06427334443527, 16.118206312684986),
            ),
            (
                (3.0, -1.0),
                (-10.0, 1.0),
                false,
                true,
                (-18273490.517386872, -13867209.344987316),
            ),
            (
                (0.0, 1.0),
                (-1.0, 0.0),
                true,
                true,
                (0.10937464735345184, 0.10012138808909089),
            ),
            (
                (0.0, 1.0),
                (-5.0, 0.0),
                false,
                true,
                (3.8871876338899116, -0.012283259469378822),
            ),
            (
                (0.0, 1.0),
                (-3.0, -4.0),
                true,
                true,
                (63.66034130312379, 1.428717870574448),
            ),
            (
                (0.0, 1.0),
                (-10.0, 1.0),
                false,
                true,
                (228.37431774231862, 4.329835343049411),
            ),
            (
                (7.25, 0.0),
                (-1.0, 0.0),
                true,
                true,
                (-0.00020438861837374537, -0.00020438861837374537),
            ),
            (
                (7.25, 0.0),
                (-5.0, 0.0),
                false,
                true,
                (896.4829392179655, 895.4829392179655),
            ),
            (
                (7.25, 0.0),
                (-3.0, -4.0),
                true,
                true,
                (190.02199161274027, 28.837856501221438),
            ),
            (
                (7.25, 0.0),
                (-10.0, 1.0),
                false,
                true,
                (13434553.448929822, -16212331.323305843),
            ),
            (
                (0.0, 0.0),
                (0.1, 0.1),
                false,
                false,
                (1.4786888267236136, -0.690287117123823),
            ),
            (
                (-4.0, 9.0),
                (-0.2, 4.3),
                true,
                false,
                (3.061241286078383e-10, 6.83455267220194e-11),
            ),
            (
                (30.0, 0.0),
                (-0.8715574274765824, 9.961946980917455),
                true,
                false,
                (-5.394476981071017e+28, 5.085025250108756e+28),
            ),
            (
                (0.0, 0.0),
                (-40.0, 0.0),
                false,
                false,
                (-6039718263611242.0, -3.141592653589793),
            ),
            (
                (-2.0, 0.0),
                (-17.0, 0.0),
                false,
                false,
                (-6088.58445316957, -1.5707963267948966),
            ),
            (
                (-3.0, 0.0),
                (-12.0, 0.0),
                false,
                false,
                (13.003028870786109, 0.5235987755982989),
            ),
            (
                (0.5, 0.0),
                (-2.5, 30.0),
                false,
                false,
                (1.8268002989101217, 1.264770083532028),
            ),
            (
                (50.0, 5.0),
                (-90.0, 0.0),
                true,
                false,
                (-6.054503947321104e+127, -3.0897344383458527e+127),
            ),
        ];
        for &(a, z, lower, reg, want) in rows {
            let got = igamma(cx(a.0, a.1), cx(z.0, z.1), lower, reg);
            let w = cx(want.0, want.1);
            assert!(
                got.sub(w).abs() <= 1e-12 * w.abs(),
                "Γ({a:?}, {z:?}, lower {lower}, reg {reg}) = {got:?}, mpmath {want:?}"
            );
        }
    }

    /// COMPLEX's text form (Excel and LibreOffice), and exponents in parsed
    /// numbers.
    /// Legendre's continued fraction for Γ(a, z), Re z > 0 past
    /// [`upper_cf_edge`], against mpmath 1.3's gammainc (30 digits). The
    /// old 1 − P route gave Q(½, 30 + i) as 1.2e-5 + 3.4e-5i (as Gnumeric
    /// 1.12 does) and γ(1 + i, 40) as the regularized 1.
    #[test]
    fn upper_fraction_matches_mpmath() {
        let near = |got: Cx, want: (f64, f64)| {
            let err = got.sub(cx(want.0, want.1)).abs();
            assert!(
                err <= 1e-13 * cx(want.0, want.1).abs(),
                "{got:?} vs {want:?}"
            );
        };
        for (a, z, lower, reg, want) in [
            // The reported cancellations, regularized upper.
            (
                (0.5, 0.0),
                (30.0, 1.0),
                false,
                true,
                (4.994_315_590_065_452e-15, -8.061_578_686_975_159e-15),
            ),
            (
                (0.5, 0.0),
                (20.0, 1.0),
                false,
                true,
                (1.320_028_044_795_353_4e-10, -2.167_919_261_241_570_4e-10),
            ),
            // Its complement, regularized lower.
            (
                (0.5, 0.0),
                (30.0, 1.0),
                true,
                true,
                (0.999_999_999_999_995, 8.061_578_686_975_159e-15),
            ),
            // Unregularized upper and lower, a off the real axis.
            (
                (2.0, 1.0),
                (10.0, 0.0),
                true,
                false,
                (0.653_332_790_054_432_5, 0.342_730_490_552_389_63),
            ),
            (
                (0.0, 1.0),
                (5.0, 5.0),
                false,
                true,
                (-1.687_983_304_016_848_3e-4, -7.933_469_441_132_255e-4),
            ),
            // Re a < 0, where the edge shrinks with |Im a| / |Re a|.
            (
                (-2.5, 0.0),
                (3.0, 4.0),
                false,
                false,
                (9.637_012_829_857_608e-5, -7.072_565_277_530_909e-5),
            ),
            (
                (-20.0, 5.0),
                (3.0, 1.0),
                false,
                true,
                (-48_661_677_983.185_36, -34_550_116_066.223),
            ),
            // Γ(0, z) = E₁(z), which the lower series cannot give (no Γ(0)).
            (
                (0.0, 0.0),
                (1.0, 1.0),
                false,
                false,
                (2.816_244_519_814_183_4e-4, -0.179_324_535_039_358_94),
            ),
            // z = a − 1 zeroes the fraction's first denominator; Lentz's
            // seed 1/0 is infinite, so this falls back to the lower series.
            (
                (1.5, 10.0),
                (0.5, 10.0),
                false,
                true,
                (0.561_591_846_888_306, -0.057_111_340_012_525_824),
            ),
        ] {
            near(igamma(cx(a.0, a.1), cx(z.0, z.1), lower, reg), want);
        }
        // After the asymptotic upper series, the unregularized lower value
        // is Γ(a)·(1 − Q), not 1 − Γ(a)·Q.
        near(
            igamma(cx(1.0, 1.0), cx(40.0, 0.0), true, false),
            (0.498_015_668_118_356_07, -0.154_949_828_301_810_67),
        );
        // The edge: Re a + |Im a|/2 for Re a ≥ 0, shrunk for Re a < 0,
        // never under ¼; the fraction declines at or inside it and for
        // Re z ≤ 0.
        assert_eq!(upper_cf_edge(cx(2.0, -6.0)), 5.0);
        assert_eq!(upper_cf_edge(cx(-4.0, 2.0)), 0.5);
        assert_eq!(upper_cf_edge(cx(-1.0, 4.0)), 2.0);
        assert_eq!(upper_cf_edge(cx(0.1, 0.0)), 0.25);
        assert!(igamma_upper_cf(cx(2.0, -6.0), cx(3.0, -4.0)).is_none());
        assert!(igamma_upper_cf(cx(2.0, 0.0), cx(-30.0, 1.0)).is_none());
        // A fraction that has not settled by 10 000 terms (a ≈ z ≈ 1e11
        // needs about 41 000) gives up.
        assert!(igamma_upper_cf(cx(1e11, 1e-3), cx(1e11 + 1.0, 0.0)).is_none());
        // Just outside the edge both routes agree: Q from the fraction and
        // 1 − P from the lower series, a out to |a| = 30 all round the
        // plane and z all round the right half-plane.
        for am in [0.3, 1.0, 5.0, 30.0] {
            for k in 0..12 {
                let t = PI * (f64::from(k) / 6.0 - 1.0) + 0.1;
                let a = cx(am * t.cos(), am * t.sin());
                for j in [-1.4, -0.7, 0.0, 0.7, 1.4] {
                    let r = upper_cf_edge(a) * (1.0 + 1e-15);
                    let z = cx(r * f64::cos(j), r * f64::sin(j));
                    if upper_asymp(a, z).is_some() {
                        continue;
                    }
                    let g = gamma(a);
                    let q = igamma_upper_cf(a, z).unwrap().div(g);
                    let one_minus_p = ONE.sub(igamma_lower_cf(a, z).unwrap().div(g));
                    assert!(
                        q.sub(one_minus_p).abs() <= 2e-13 * q.abs(),
                        "a={a:?} z={z:?}: {q:?} vs {one_minus_p:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn shows_and_parses_like_excel() {
        let t = |re, im| show(cx(re, im), None);
        let ok = |v: &str| Ok(CellValue::Text(v.into()));
        assert_eq!(t(1.0, 1.0), ok("1+i"));
        assert_eq!(t(0.0, 1.0), ok("i"));
        assert_eq!(t(0.0, -1.0), ok("-i"));
        assert_eq!(t(1.0, -1.0), ok("1-i"));
        assert_eq!(t(2.0, -3.0), ok("2-3i"));
        assert_eq!(t(0.0, 2.5), ok("2.5i"));
        assert_eq!(t(1.0, 2.5), ok("1+2.5i"));
        assert_eq!(t(-0.0, 0.0), ok("0"));
        assert_eq!(t(f64::INFINITY, 0.0), Err(err::NUM));
        assert_eq!(t(0.0, f64::NAN), Err(err::NUM));
        let p = |s: &str| parse(s).map(|(z, u)| (z.re, z.im, u));
        assert_eq!(p("1E5"), Some((1e5, 0.0, None)));
        assert_eq!(p("1.e3"), Some((1e3, 0.0, None)));
        assert_eq!(p("2e+3i"), Some((0.0, 2e3, Some('i'))));
        assert_eq!(p("1e+"), None);
        assert_eq!(p("1e-x"), None);
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
