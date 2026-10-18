//! LibreOffice's barrier pricing (`scaddins/source/pricing`): OPT_BARRIER,
//! OPT_TOUCH, OPT_PROB_HIT and OPT_PROB_INMONEY (ids 1800–1803), with the
//! Greeks LibreOffice offers. Every price is built from two binaries
//! (cash-or-nothing and asset-or-nothing); a single barrier by the reflection
//! principle, a double barrier by its image series.

use super::super::statistical::{norm_cdf as pnorm, norm_pdf as dnorm};
use super::super::{err, n, ok_num, t, R};
use super::Side::{self, Call, Put};
use crate::CellValue;
use Greek::{Delta, Gamma, RhoD, RhoF, Theta, Value, Vanna, Vega, Volga};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Greek {
    Value,
    Delta,
    Gamma,
    Theta,
    Vega,
    Volga,
    Vanna,
    RhoD,
    RhoF,
}

/// Volatility, domestic and foreign rates, time to expiry.
#[derive(Clone, Copy)]
struct Mkt {
    vol: f64,
    rd: f64,
    rf: f64,
    tau: f64,
}

/// A (possibly barrier-truncated) payoff: strike `k` (negative: a binary
/// paying between the barriers), barriers `b1` below and `b2` above (≤ 0:
/// none), and whether a binary pays the asset (foreign) or cash.
#[derive(Clone, Copy)]
struct Opt {
    k: f64,
    b1: f64,
    b2: f64,
    pc: Side,
    foreign: bool,
}

fn sqr(x: f64) -> f64 {
    x * x
}

impl Mkt {
    fn d1d2(self, s: f64, k: f64) -> (f64, f64) {
        let sv = self.vol * self.tau.sqrt();
        let d1 = ((s / k).ln() + (self.rd - self.rf + 0.5 * self.vol * self.vol) * self.tau) / sv;
        (d1, d1 - sv)
    }
}

fn in_money(pc: Side, s: f64, k: f64) -> bool {
    (pc == Call && s >= k) || (pc == Put && s <= k)
}

/// Pays 1 (domestic) if the spot ends beyond `k`.
fn bincash(s: f64, m: Mkt, k: f64, pc: Side, g: Greek) -> f64 {
    let Mkt { vol, rd, tau, .. } = m;
    let val = if tau <= 0.0 {
        f64::from(u8::from(g == Value && in_money(pc, s, k)))
    } else if k == 0.0 {
        match (pc, g) {
            (Put, _) => 0.0,
            (_, Value) => 1.0,
            (_, Theta) => rd,
            (_, RhoD) => -tau,
            _ => 0.0,
        }
    } else {
        let (d1, d2) = m.d1d2(s, k);
        let pm = pc.z();
        let st = tau.sqrt();
        match g {
            Value => pnorm(pm * d2),
            Delta => pm * dnorm(d2) / (s * vol * st),
            Gamma => -pm * dnorm(d2) * d1 / (sqr(s * vol) * tau),
            Theta => {
                rd * pnorm(pm * d2) + pm * dnorm(d2) * ((s / k).ln() / (vol * st) - 0.5 * d2) / tau
            }
            Vega => -pm * dnorm(d2) * d1 / vol,
            Volga => pm * dnorm(d2) / (vol * vol) * (-d1 * d1 * d2 + d1 + d2),
            Vanna => pm * dnorm(d2) / (s * vol * vol * st) * (d1 * d2 - 1.0),
            RhoD => -tau * pnorm(pm * d2) + pm * dnorm(d2) * st / vol,
            RhoF => -pm * dnorm(d2) * st / vol,
        }
    };
    (-rd * tau).exp() * val
}

/// Pays the asset (one unit of foreign) if the spot ends beyond `k`.
fn binasset(s: f64, m: Mkt, k: f64, pc: Side, g: Greek) -> f64 {
    let Mkt { vol, rf, tau, .. } = m;
    let val = if tau <= 0.0 {
        match g {
            Value if in_money(pc, s, k) => s,
            Delta if in_money(pc, s, k) => 1.0,
            _ => 0.0,
        }
    } else if k == 0.0 {
        match (pc, g) {
            (Put, _) => 0.0,
            (_, Value) => s,
            (_, Delta) => 1.0,
            (_, Theta) => rf * s,
            (_, RhoF) => -tau * s,
            _ => 0.0,
        }
    } else {
        let (d1, d2) = m.d1d2(s, k);
        let pm = pc.z();
        let st = tau.sqrt();
        match g {
            Value => s * pnorm(pm * d1),
            Delta => pnorm(pm * d1) + pm * dnorm(d1) / (vol * st),
            Gamma => -pm * dnorm(d1) * d2 / (s * sqr(vol) * tau),
            Theta => {
                rf * s * pnorm(pm * d1)
                    + pm * s * dnorm(d1) * ((s / k).ln() / (vol * st) - 0.5 * d1) / tau
            }
            Vega => -pm * s * dnorm(d1) * d2 / vol,
            Volga => pm * s * dnorm(d1) / (vol * vol) * (-d1 * d2 * d2 + d1 + d2),
            Vanna => pm * dnorm(d1) / (vol * vol * st) * (d2 * d2 - 1.0),
            RhoD => pm * s * dnorm(d1) * st / vol,
            RhoF => -tau * s * pnorm(pm * d1) - pm * s * dnorm(d1) * st / vol,
        }
    };
    (-rf * tau).exp() * val
}

fn binary(s: f64, m: Mkt, k: f64, pc: Side, foreign: bool, g: Greek) -> f64 {
    if foreign {
        binasset(s, m, k, pc, g)
    } else {
        bincash(s, m, k, pc, g)
    }
}

/// A binary paying if the spot ends between `b1` and `b2`.
fn binary_range(s: f64, m: Mkt, b1: f64, b2: f64, foreign: bool, g: Greek) -> f64 {
    if b1 <= 0.0 && b2 <= 0.0 {
        binary(s, m, 0.0, Call, foreign, g)
    } else if b1 <= 0.0 {
        binary(s, m, b2, Put, foreign, g)
    } else if b2 <= 0.0 {
        binary(s, m, b1, Call, foreign, g)
    } else if b2 <= b1 {
        0.0
    } else {
        binary(s, m, b2, Put, foreign, g) - binary(s, m, b1, Put, foreign, g)
    }
}

fn putcall(s: f64, m: Mkt, k: f64, pc: Side, g: Greek) -> f64 {
    let Mkt { vol, rd, rf, tau } = m;
    let pm = pc.z();
    let split = || pm * (binasset(s, m, k, pc, g) - k * bincash(s, m, k, pc, g));
    if k == 0.0 || tau == 0.0 {
        return split();
    }
    let (d1, d2) = m.d1d2(s, k);
    match g {
        Value => {
            pm * ((-rf * tau).exp() * s * pnorm(pm * d1) - (-rd * tau).exp() * k * pnorm(pm * d2))
        }
        Delta => pm * (-rf * tau).exp() * pnorm(pm * d1),
        Gamma => (-rf * tau).exp() * dnorm(d1) / (s * vol * tau.sqrt()),
        _ => split(),
    }
}

/// A put or call paying only on one side of barrier `b`: `regular` keeps the
/// payoff beyond the barrier, otherwise the part between strike and barrier.
fn putcalltrunc1(s: f64, m: Mkt, k: f64, b: f64, pc: Side, regular: bool, g: Greek) -> f64 {
    let plain = (pc == Call && b <= k) || (pc == Put && b >= k);
    if regular {
        if plain {
            putcall(s, m, k, pc, g)
        } else {
            pc.z() * (binasset(s, m, b, pc, g) - k * bincash(s, m, b, pc, g))
        }
    } else if plain {
        0.0
    } else {
        binasset(s, m, k, Call, g)
            - binasset(s, m, b, Call, g)
            - k * (bincash(s, m, k, Call, g) - bincash(s, m, b, Call, g))
    }
}

/// A put or call paying only if the spot ends between the barriers.
fn putcalltrunc(s: f64, m: Mkt, o: Opt, g: Greek) -> f64 {
    let Opt { k, b1, b2, pc, .. } = o;
    if b1 <= 0.0 && b2 <= 0.0 {
        putcall(s, m, k, pc, g)
    } else if b1 <= 0.0 {
        putcalltrunc1(s, m, k, b2, pc, pc == Put, g)
    } else if b2 <= 0.0 {
        putcalltrunc1(s, m, k, b1, pc, pc == Call, g)
    } else if b2 <= b1 {
        0.0
    } else {
        pc.z() * (putcalltrunc1(s, m, k, b1, pc, true, g) - putcalltrunc1(s, m, k, b2, pc, true, g))
    }
}

fn vanilla(s: f64, m: Mkt, o: Opt, g: Greek) -> f64 {
    if o.k < 0.0 {
        binary_range(s, m, o.b1, o.b2, o.foreign, g)
    } else {
        putcall(s, m, o.k, o.pc, g)
    }
}

fn vanilla_trunc(s: f64, m: Mkt, o: Opt, g: Greek) -> f64 {
    if o.k < 0.0 {
        binary_range(s, m, o.b1, o.b2, o.foreign, g)
    } else {
        putcalltrunc(s, m, o, g)
    }
}

/// W(S) = V(sc·S) − (B/S)^a V(sc·B²/S): a truncated vanilla minus its
/// reflection in the (single) barrier, and that expression's Greeks.
fn barrier_term(s: f64, m: Mkt, o: Opt, sc: f64, g: Greek) -> f64 {
    let Mkt { vol, rd, rf, .. } = m;
    let b = if o.b1 > 0.0 { o.b1 } else { o.b2 };
    let a = 2.0 * (rd - rf) / (vol * vol) - 1.0;
    let bb = 4.0 * (rd - rf) / (vol * vol * vol); // −da/dvol
    let c = 12.0 * (rd - rf) / (vol * vol * vol * vol); // −db/dvol
    let vt = |x: f64, g: Greek| vanilla_trunc(x, m, o, g);
    let refl = sc * b * b / s;
    let pa = (b / s).powf(a);
    let lb = (b / s).ln();
    match g {
        Value | Theta => vt(sc * s, g) - pa * vt(refl, g),
        Delta => {
            sc * vt(sc * s, g) + pa * (a / s * vt(refl, Value) + sqr(b / s) * sc * vt(refl, g))
        }
        Gamma => {
            sc * sc * vt(sc * s, g)
                - pa * (a * (a + 1.0) / (s * s) * vt(refl, Value)
                    + (2.0 * a + 2.0) * b * b / (s * s * s) * sc * vt(refl, Delta)
                    + sqr(sqr(b / s)) * sc * sc * vt(refl, Gamma))
        }
        Vega => vt(sc * s, g) - pa * (-bb * lb * vt(refl, Value) + vt(refl, g)),
        Volga => {
            vt(sc * s, g)
                - pa * (lb * (bb * bb * lb + c) * vt(refl, Value) - 2.0 * bb * lb * vt(refl, Vega)
                    + vt(refl, Volga))
        }
        Vanna => {
            let refl = b * b / s * sc;
            sc * vt(sc * s, g)
                - pa * (bb / s * (lb * a + 1.0) * vt(refl, Value)
                    + bb * lb * sqr(b / s) * sc * vt(refl, Delta)
                    - a / s * vt(refl, Vega)
                    - sqr(b / s) * sc * vt(refl, Vanna))
        }
        RhoD => vt(sc * s, g) - pa * (2.0 * lb / (vol * vol) * vt(refl, Value) + vt(refl, g)),
        RhoF => vt(sc * s, g) - pa * (-2.0 * lb / (vol * vol) * vt(refl, Value) + vt(refl, g)),
    }
}

/// Image `i` of the double-barrier series, weighted `fac` = (B2/B1)^(i·a).
fn barrier_double_term(s: f64, m: Mkt, o: Opt, fac: f64, sc: f64, i: i32, g: Greek) -> f64 {
    let Mkt { vol, rd, rf, .. } = m;
    let i = f64::from(i);
    let b = 4.0 * i * (rd - rf) / (vol * vol * vol);
    let c = 12.0 * i * (rd - rf) / (vol * vol * vol * vol);
    let bt = |g: Greek| barrier_term(s, m, o, sc, g);
    let l = (o.b2 / o.b1).ln();
    match g {
        Value | Delta | Gamma | Theta => fac * bt(g),
        Vega => fac * bt(g) - b * l * fac * bt(Value),
        Volga => fac * bt(g) - 2.0 * b * l * fac * bt(Vega) + l * fac * (c + b * b * l) * bt(Value),
        Vanna => fac * bt(g) - b * l * fac * bt(Delta),
        RhoD => fac * bt(g) + 2.0 * i / (vol * vol) * l * fac * bt(Value),
        RhoF => fac * bt(g) - 2.0 * i / (vol * vol) * l * fac * bt(Value),
    }
}

/// A continuously monitored knock-out.
fn barrier_ko(s: f64, m: Mkt, o: Opt, g: Greek) -> f64 {
    let Opt { b1, b2, .. } = o;
    if b1 <= 0.0 && b2 <= 0.0 {
        return vanilla(s, m, o, g);
    }
    if b2 <= 0.0 {
        return if s <= b1 {
            0.0
        } else {
            barrier_term(s, m, o, 1.0, g)
        };
    }
    if b1 <= 0.0 {
        return if s >= b2 {
            0.0
        } else {
            barrier_term(s, m, o, 1.0, g)
        };
    }
    if s <= b1 || s >= b2 {
        return 0.0; // knocked out (always when b1 ≥ b2)
    }
    let a = 2.0 * (m.rd - m.rf) / (m.vol * m.vol) - 1.0;
    let (bb2, bba) = (sqr(b2 / b1), (b2 / b1).powf(a));
    let (bb2inv, bbainv) = (1.0 / bb2, 1.0 / bba);
    let (mut fac, mut facinv, mut sc, mut scinv) = (1.0, 1.0, 1.0, 1.0);
    let mut val = barrier_double_term(s, m, o, fac, sc, 0, g);
    for i in 1..10 {
        fac *= bba;
        facinv *= bbainv;
        sc *= bb2;
        scinv *= bb2inv;
        let add = barrier_double_term(s, m, o, fac, sc, i, g)
            + barrier_double_term(s, m, o, facinv, scinv, -i, g);
        val += add;
        if add.abs() <= 1e-12 * val.abs() {
            break;
        }
    }
    val
}

/// Knock-in or knock-out, monitored continuously or at expiry only.
fn knock(s: f64, m: Mkt, o: Opt, knock_in: bool, continuous: bool, g: Greek) -> f64 {
    let out = if continuous {
        barrier_ko(s, m, o, g)
    } else {
        vanilla_trunc(s, m, o, g)
    };
    if knock_in {
        vanilla(
            s,
            m,
            Opt {
                b1: -1.0,
                b2: -1.0,
                ..o
            },
            g,
        ) - out
    } else {
        out
    }
}

/// The probability of ending between the barriers (rd = 0, rf = −mu).
fn prob_in_money(s: f64, m: Mkt, b1: f64, b2: f64) -> f64 {
    binary_range(s, m, b1, b2, false, Value)
}

/// The first character of text argument `i`: `yes` → true, `no` → false
/// (LibreOffice is case-sensitive here).
fn flag(a: &[CellValue], i: usize, yes: char, no: char) -> Result<bool, u8> {
    let s = t(a, i)?;
    if s.starts_with(yes) {
        Ok(true)
    } else if s.starts_with(no) {
        Ok(false)
    } else {
        Err(err::NUM)
    }
}

fn put_call(a: &[CellValue], i: usize) -> Result<Side, u8> {
    Ok(if flag(a, i, 'c', 'p')? { Call } else { Put })
}

fn greek(a: &[CellValue], i: usize) -> Result<Greek, u8> {
    if a.len() <= i {
        return Ok(Value);
    }
    Ok(match t(a, i)?.as_str() {
        "value" | "price" | "v" | "p" => Value,
        "delta" | "d" => Delta,
        "gamma" | "g" => Gamma,
        "theta" | "t" => Theta,
        "vega" | "e" => Vega,
        "volga" | "o" => Volga,
        "vanna" | "a" => Vanna,
        "rho" | "r" => RhoD,
        "rhof" | "f" => RhoF,
        _ => return Err(err::NUM),
    })
}

/// Evaluate function `id` (1800–1803).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    let x = |i: usize| n(a, i);
    let (s, vol) = (x(0)?, x(1)?);
    let bad = |tau: f64| s <= 0.0 || vol <= 0.0 || tau < 0.0;
    let v = match id {
        1800 => {
            let (rd, rf, tau, k) = (x(2)?, x(3)?, x(4)?, x(5)?);
            let (b1, b2, rebate) = (x(6)?, x(7)?, x(8)?);
            let pc = put_call(a, 9)?;
            let (knock_in, continuous) = (flag(a, 10, 'i', 'o')?, flag(a, 11, 'c', 'e')?);
            let g = greek(a, 12)?;
            if bad(tau) || k < 0.0 {
                return Err(err::NUM);
            }
            let m = Mkt { vol, rd, rf, tau };
            let o = Opt {
                k,
                b1,
                b2,
                pc,
                foreign: false,
            };
            let mut v = knock(s, m, o, knock_in, continuous, g);
            if rebate != 0.0 {
                // The rebate is paid when the option does not pay.
                let touch = Opt {
                    k: -1.0,
                    pc: Call,
                    ..o
                };
                v += rebate * knock(s, m, touch, !knock_in, continuous, g);
            }
            v
        }
        1801 => {
            let (rd, rf, tau, b1, b2) = (x(2)?, x(3)?, x(4)?, x(5)?, x(6)?);
            let foreign = flag(a, 7, 'f', 'd')?;
            let (knock_in, continuous) = (flag(a, 8, 'i', 'o')?, flag(a, 9, 'c', 'e')?);
            let g = greek(a, 10)?;
            if bad(tau) {
                return Err(err::NUM);
            }
            let o = Opt {
                k: -1.0,
                b1,
                b2,
                pc: Call,
                foreign,
            };
            knock(s, Mkt { vol, rd, rf, tau }, o, knock_in, continuous, g)
        }
        1802 => {
            let (mu, tau, b1, b2) = (x(2)?, x(3)?, x(4)?, x(5)?);
            if bad(tau) {
                return Err(err::NUM);
            }
            let m = Mkt {
                vol,
                rd: 0.0,
                rf: -mu,
                tau,
            };
            let o = Opt {
                k: -1.0,
                b1,
                b2,
                pc: Call,
                foreign: false,
            };
            1.0 - knock(s, m, o, false, true, Value)
        }
        1803 => {
            let (mu, tau, b1, b2) = (x(2)?, x(3)?, x(4)?, x(5)?);
            let k = if a.len() > 6 { x(6)? } else { -1.0 };
            let pc = if a.len() > 7 { put_call(a, 7)? } else { Call };
            if bad(tau) {
                return Err(err::NUM);
            }
            let m = Mkt {
                vol,
                rd: 0.0,
                rf: -mu,
                tau,
            };
            if k < 0.0 {
                prob_in_money(s, m, b1, b2)
            } else if (b1 > b2 && b2 > 0.0)
                || (k >= b2 && b2 > 0.0 && pc == Call)
                || (k <= b1 && pc == Put)
            {
                0.0
            } else if pc == Call {
                prob_in_money(s, m, b1.max(k), b2)
            } else {
                prob_in_money(s, m, b1, if b2 > 0.0 { b2.min(k) } else { k })
            }
        }
        _ => return Err(err::NAME),
    };
    ok_num(v)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// OPT_BARRIER, OPT_TOUCH and OPT_PROB_* through `eval`, against
    /// LibreOffice 25.2's pricing add-in (GETOPTBARRIER, …).
    fn call(id: u32, nums: &[f64], flags: &[&str]) -> R {
        let mut a: Vec<CellValue> = nums.iter().map(|x| CellValue::num(*x)).collect();
        a.extend(flags.iter().map(|f| CellValue::Text((*f).into())));
        eval(id, &a)
    }

    #[test]
    fn eval_matches_libreoffice() {
        let near = |got: R, want: f64| {
            let g = got.unwrap().as_num().unwrap();
            assert!(
                (g - want).abs() <= 1e-12 * want.abs().max(1.0),
                "{g} vs {want}"
            );
        };
        let mkt = [30.0, 0.2, 0.06, 0.02];
        let bar = |t: f64, k: f64, b1: f64, b2: f64, rebate: f64, flags: &[&str]| {
            let mut v = mkt.to_vec();
            v.extend([t, k, b1, b2, rebate]);
            call(1800, &v, flags)
        };
        // At expiry: a put struck below the spot pays nothing; a call's
        // delta is 1. A zero strike, a rebate on a knock-in.
        near(bar(0.0, 25.0, 20.0, 0.0, 0.0, &["p", "o", "c"]), 0.0);
        near(
            bar(0.0, 25.0, 20.0, 0.0, 0.0, &["c", "o", "c", "delta"]),
            1.0,
        );
        near(
            bar(1.0, 0.0, 20.0, 0.0, 0.0, &["c", "o", "c"]),
            28.746_249_329_397_372,
        );
        near(
            bar(1.0, 25.0, 20.0, 0.0, 1.5, &["c", "i", "c"]),
            1.364_748_307_397_057_2,
        );
        // Knocked out already: the spot above the upper barrier, or outside
        // a double barrier.
        near(bar(1.0, 25.0, 0.0, 28.0, 0.0, &["c", "o", "c"]), 0.0);
        near(bar(1.0, 25.0, 35.0, 40.0, 0.0, &["c", "o", "c"]), 0.0);
        near(bar(1.0, 25.0, 0.0, 20.0, 0.0, &["p", "o", "c"]), 0.0);
        near(
            bar(1.0, 25.0, 0.0, 20.0, 0.0, &["c", "i", "c"]),
            6.213_586_521_226_453,
        );
        near(
            bar(1.0, 25.0, 20.0, 40.0, 0.0, &["p", "o", "c", "gamma"]),
            0.005_967_159_136_384_383,
        );
        // Err:502 there: a negative strike or time.
        assert_eq!(
            bar(1.0, -1.0, 20.0, 0.0, 0.0, &["c", "o", "c"]),
            Err(err::NUM)
        );
        assert_eq!(
            bar(-1.0, 25.0, 20.0, 0.0, 0.0, &["c", "o", "c"]),
            Err(err::NUM)
        );
        let touch = [30.0, 0.2, 0.06, 0.02, -1.0, 25.0, 0.0];
        assert_eq!(call(1801, &touch, &["d", "i", "c"]), Err(err::NUM));
        let touch = [30.0, 0.2, 0.06, 0.02, 0.5, 25.0, 0.0];
        near(
            call(1801, &touch, &["d", "i", "c"]),
            0.174_543_429_435_836_23,
        );
        assert_eq!(
            call(1802, &[30.0, 0.2, 0.1, -1.0, 20.0, 0.0], &[]),
            Err(err::NUM)
        );
        let pim = |t: f64, b1: f64, b2: f64, k: f64, pc: &str| {
            call(1803, &[30.0, 0.2, 0.1, t, b1, b2, k], &[pc])
        };
        assert_eq!(
            call(1803, &[30.0, 0.2, 0.1, -1.0, 25.0, 35.0], &[]),
            Err(err::NUM)
        );
        // In the money and between the barriers: none when the strike is
        // past the barrier on the paying side, or the barriers cross.
        near(pim(1.0, 0.0, 35.0, 40.0, "c"), 0.0);
        near(pim(1.0, 25.0, 0.0, 20.0, "p"), 0.0);
        near(pim(1.0, 35.0, 25.0, 28.0, "c"), 0.0);
        near(pim(1.0, 25.0, 35.0, 28.0, "p"), 0.133_320_380_934_200_95);
        near(pim(1.0, 0.0, 0.0, 28.0, "p"), 0.228_146_633_586_407_58);
        near(pim(1.0, 25.0, 35.0, 40.0, "p"), 0.549_763_140_586_935_7);
        near(pim(1.0, 25.0, 35.0, 20.0, "c"), 0.549_763_140_586_935_7);
        near(pim(1.0, 25.0, 0.0, 20.0, "c"), 0.905_173_747_347_793_4);
        near(pim(1.0, 0.0, 35.0, 40.0, "p"), 0.644_589_393_239_142_3);
        near(pim(1.0, 35.0, 25.0, 28.0, "p"), 0.0);
        let hit = [-30.0, 0.2, 0.1, 1.0, 20.0, 0.0];
        assert_eq!(call(1802, &hit, &[]), Err(err::NUM));
        // A zero strike and no barriers: the forward; end-monitored
        // barriers (truncated payoffs); a spot between or outside a
        // double barrier, and crossed barriers.
        near(
            bar(1.0, 0.0, 0.0, 0.0, 0.0, &["c", "o", "c"]),
            29.405_960_199_202_656,
        );
        near(
            bar(1.0, 25.0, 0.0, 0.0, 0.0, &["c", "o", "e"]),
            6.213_586_521_226_453,
        );
        near(bar(1.0, 25.0, 0.0, 20.0, 0.0, &["c", "o", "e"]), 0.0);
        near(
            bar(1.0, 25.0, 28.0, 40.0, 0.0, &["c", "o", "c"]),
            1.314_607_241_506_555_3,
        );
        near(bar(1.0, 25.0, 20.0, 28.0, 0.0, &["c", "o", "c"]), 0.0);
        near(bar(1.0, 25.0, 40.0, 20.0, 0.0, &["c", "o", "c"]), 0.0);
    }

    const M: Mkt = Mkt {
        vol: 0.2,
        rd: 0.06,
        rf: 0.02,
        tau: 0.5,
    };

    /// Every Greek of a binary is the derivative of its value (central
    /// differences; theta is −∂/∂τ), across the expiry, zero-strike and general branches.
    #[test]
    fn binary_greeks_are_derivatives() {
        let h = 1e-5;
        for (pc, k) in [(Call, 28.0), (Put, 28.0), (Call, 0.0), (Put, 0.0)] {
            for f in [bincash, binasset] {
                let at = |s: f64, m: Mkt| f(s, m, k, pc, Value);
                let s = 30.0;
                let fd = [
                    (Delta, (at(s + h, M) - at(s - h, M)) / (2.0 * h)),
                    (
                        Vega,
                        (at(
                            s,
                            Mkt {
                                vol: M.vol + h,
                                ..M
                            },
                        ) - at(
                            s,
                            Mkt {
                                vol: M.vol - h,
                                ..M
                            },
                        )) / (2.0 * h),
                    ),
                    (
                        RhoD,
                        (at(s, Mkt { rd: M.rd + h, ..M }) - at(s, Mkt { rd: M.rd - h, ..M }))
                            / (2.0 * h),
                    ),
                    (
                        RhoF,
                        (at(s, Mkt { rf: M.rf + h, ..M }) - at(s, Mkt { rf: M.rf - h, ..M }))
                            / (2.0 * h),
                    ),
                    (
                        Theta,
                        -(at(
                            s,
                            Mkt {
                                tau: M.tau + h,
                                ..M
                            },
                        ) - at(
                            s,
                            Mkt {
                                tau: M.tau - h,
                                ..M
                            },
                        )) / (2.0 * h),
                    ),
                ];
                for (g, want) in fd {
                    let got = f(s, M, k, pc, g);
                    assert!(
                        (got - want).abs() < 1e-5 * want.abs().max(1.0),
                        "{pc:?} {k} {g:?}: {got} vs {want}"
                    );
                }
                let d = |s: f64| f(s, M, k, pc, Delta);
                let gamma = (d(s + h) - d(s - h)) / (2.0 * h);
                assert!((f(s, M, k, pc, Gamma) - gamma).abs() < 1e-5);
                for g in [Volga, Vanna] {
                    assert!(f(s, M, k, pc, g).is_finite());
                }
            }
        }
        // At expiry: the payoff, its delta, and no other sensitivity.
        let m0 = Mkt { tau: 0.0, ..M };
        assert_eq!(bincash(30.0, m0, 28.0, Call, Value), 1.0);
        assert_eq!(bincash(30.0, m0, 28.0, Put, Value), 0.0);
        assert_eq!(bincash(30.0, m0, 28.0, Call, Delta), 0.0);
        assert_eq!(binasset(30.0, m0, 28.0, Call, Value), 30.0);
        assert_eq!(binasset(30.0, m0, 28.0, Put, Delta), 0.0);
        assert_eq!(binasset(30.0, m0, 28.0, Call, Delta), 1.0);
        assert_eq!(binasset(30.0, m0, 28.0, Call, Gamma), 0.0);
    }

    #[test]
    fn every_greek_of_every_barrier_shape() {
        let greeks = [Value, Delta, Gamma, Theta, Vega, Volga, Vanna, RhoD, RhoF];
        for (b1, b2) in [
            (-1.0, -1.0),
            (20.0, 0.0),
            (0.0, 40.0),
            (20.0, 40.0),
            (40.0, 20.0),
        ] {
            for k in [-1.0, 25.0, 35.0] {
                for pc in [Call, Put] {
                    for foreign in [false, true] {
                        let o = Opt {
                            k,
                            b1,
                            b2,
                            pc,
                            foreign,
                        };
                        for g in greeks {
                            for (ki, cont) in
                                [(false, false), (false, true), (true, false), (true, true)]
                            {
                                assert!(knock(30.0, M, o, ki, cont, g).is_finite());
                            }
                        }
                    }
                }
            }
        }
        // Knocked out already.
        let o = Opt {
            k: 25.0,
            b1: 31.0,
            b2: 0.0,
            pc: Call,
            foreign: false,
        };
        assert_eq!(barrier_ko(30.0, M, o, Value), 0.0);
        assert_eq!(
            barrier_ko(
                30.0,
                M,
                Opt {
                    b1: 0.0,
                    b2: 29.0,
                    ..o
                },
                Value
            ),
            0.0
        );
        assert_eq!(
            barrier_ko(
                30.0,
                M,
                Opt {
                    b1: 31.0,
                    b2: 40.0,
                    ..o
                },
                Value
            ),
            0.0
        );
    }

    fn args(xs: &[f64], flags: &[&str]) -> Vec<CellValue> {
        let mut a: Vec<CellValue> = xs.iter().map(|x| CellValue::num(*x)).collect();
        a.extend(flags.iter().map(|f| CellValue::Text((*f).into())));
        a
    }

    #[test]
    fn bad_inputs() {
        let bar = [30.0, 0.2, 0.06, 0.02, 1.0, -25.0, 20.0, 0.0, 0.0];
        assert_eq!(eval(1800, &args(&bar, &["c", "o", "c"])), Err(err::NUM));
        let bar = [30.0, 0.2, 0.06, 0.02, 1.0, 25.0, 20.0, 0.0, 0.0];
        assert_eq!(eval(1800, &args(&bar, &["c", "x", "c"])), Err(err::NUM));
        assert_eq!(eval(1800, &args(&bar, &["c", "o", "x"])), Err(err::NUM));
        let tch = [30.0, 0.2, 0.06, 0.02, -1.0, 25.0, 0.0];
        assert_eq!(eval(1801, &args(&tch, &["d", "i", "c"])), Err(err::NUM));
        let pim = [30.0, 0.2, 0.1, -1.0, 25.0, 35.0];
        assert_eq!(eval(1803, &args(&pim, &[])), Err(err::NUM));
        // A put with no upper barrier is in the money up to its strike.
        let pim = [30.0, 0.2, 0.1, 1.0, 0.0, 0.0, 32.0];
        assert!(eval(1803, &args(&pim, &["p"])).is_ok());
        assert_eq!(eval(1849, &args(&pim, &[])), Err(err::NAME));
    }
}
