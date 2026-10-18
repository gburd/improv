//! CONVERT: Excel's unit table (Excel's documented spellings), with SI
//! prefixes on the metric units and binary prefixes (`ki` … `Yi`) on bits
//! and bytes. Sizes are exact definitions where one exists (the inch, the
//! pound, the US gallon, the IT calorie, …) and CODATA values otherwise.
//! Units are case-sensitive; an exact unit name beats a prefixed reading
//! (`"Pa"` is a pascal, not a peta-annum).

use super::super::{err, n, ok_num, t, R};
use crate::CellValue;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Mass,
    Length,
    Time,
    Pressure,
    Force,
    Energy,
    Power,
    Magnetism,
    Temperature,
    Volume,
    Area,
    Information,
    Speed,
}
use Class::*;

const IN: f64 = 0.0254;
const FT: f64 = 0.3048;
const MI: f64 = 1609.344;
const NMI: f64 = 1852.0;
const LY: f64 = 9_460_730_472_580_800.0;
const PICA_PT: f64 = IN / 72.0;
const LB: f64 = 453.592_37;
/// Standard gravity, m/s².
const G0: f64 = 9.806_65;
const LBF: f64 = LB / 1000.0 * G0;
/// Mechanical horsepower, 550 ft·lbf/s, in watts.
const HP: f64 = 550.0 * FT * LBF;
const TSP: f64 = 0.004_928_921_593_75;
const GAL: f64 = 3.785_411_784;

/// (name, class, size in the class's base unit, takes a prefix). Bases: g,
/// m, s, Pa, N, J, W, T, K, l, m², bit, m/s. Temperature sizes are unused.
#[rustfmt::skip]
static UNITS: &[(&str, Class, f64, bool)] = &[
    ("g", Mass, 1.0, true), ("sg", Mass, LB * G0 / FT, false), ("lbm", Mass, LB, false),
    ("u", Mass, 1.660_539_066_6e-24, true), ("ozm", Mass, LB / 16.0, false),
    ("grain", Mass, LB / 7000.0, false), ("cwt", Mass, 100.0 * LB, false),
    ("shweight", Mass, 100.0 * LB, false), ("uk_cwt", Mass, 112.0 * LB, false),
    ("lcwt", Mass, 112.0 * LB, false), ("hweight", Mass, 112.0 * LB, false),
    ("stone", Mass, 14.0 * LB, false), ("ton", Mass, 2000.0 * LB, false),
    ("uk_ton", Mass, 2240.0 * LB, false), ("LTON", Mass, 2240.0 * LB, false),
    ("brton", Mass, 2240.0 * LB, false),
    ("m", Length, 1.0, true), ("mi", Length, MI, false), ("Nmi", Length, NMI, false),
    ("in", Length, IN, false), ("ft", Length, FT, false), ("yd", Length, 3.0 * FT, false),
    ("ang", Length, 1e-10, true), ("ell", Length, 45.0 * IN, false), ("ly", Length, LY, true),
    ("parsec", Length, 3.085_677_581_491_367e16, true), ("pc", Length, 3.085_677_581_491_367e16, true),
    ("Picapt", Length, PICA_PT, false), ("Pica", Length, PICA_PT, false),
    ("pica", Length, IN / 6.0, false), ("survey_mi", Length, 6_336_000.0 / 3937.0, false),
    ("yr", Time, 31_557_600.0, false), ("day", Time, 86_400.0, false), ("d", Time, 86_400.0, false),
    ("hr", Time, 3600.0, false), ("mn", Time, 60.0, false), ("min", Time, 60.0, false),
    ("sec", Time, 1.0, true), ("s", Time, 1.0, true),
    ("Pa", Pressure, 1.0, true), ("p", Pressure, 1.0, true), ("atm", Pressure, 101_325.0, true),
    ("at", Pressure, 101_325.0, true), ("mmHg", Pressure, 133.322_387_415, true),
    ("psi", Pressure, LBF / (IN * IN), false), ("Torr", Pressure, 101_325.0 / 760.0, false),
    ("N", Force, 1.0, true), ("dyn", Force, 1e-5, true), ("dy", Force, 1e-5, true),
    ("lbf", Force, LBF, false), ("pond", Force, G0 / 1000.0, true),
    ("J", Energy, 1.0, true), ("e", Energy, 1e-7, true), ("c", Energy, 4.184, true),
    ("cal", Energy, 4.1868, true), ("eV", Energy, 1.602_176_634e-19, true),
    ("ev", Energy, 1.602_176_634e-19, true), ("HPh", Energy, HP * 3600.0, false),
    ("hh", Energy, HP * 3600.0, false), ("Wh", Energy, 3600.0, true), ("wh", Energy, 3600.0, true),
    // Excel's "foot-pound" is the foot-poundal (ft · lb · ft/s²), as in
    // LibreOffice and Gnumeric.
    ("flb", Energy, FT * LB / 1000.0 * FT, false),
    ("BTU", Energy, 1_055.055_852_62, false), ("btu", Energy, 1_055.055_852_62, false),
    ("HP", Power, HP, false), ("h", Power, HP, false), ("PS", Power, 735.498_75, false),
    ("W", Power, 1.0, true), ("w", Power, 1.0, true),
    ("T", Magnetism, 1.0, true), ("ga", Magnetism, 1e-4, true),
    ("C", Temperature, 1.0, false), ("cel", Temperature, 1.0, false),
    ("F", Temperature, 1.0, false), ("fah", Temperature, 1.0, false),
    ("K", Temperature, 1.0, true), ("kel", Temperature, 1.0, true),
    ("Rank", Temperature, 1.0, false), ("Reau", Temperature, 1.0, false),
    ("tsp", Volume, TSP, false), ("tspm", Volume, 0.005, false), ("tbs", Volume, 3.0 * TSP, false),
    ("oz", Volume, GAL / 128.0, false), ("cup", Volume, GAL / 16.0, false),
    ("pt", Volume, GAL / 8.0, false), ("us_pt", Volume, GAL / 8.0, false),
    ("uk_pt", Volume, 4.546_09 / 8.0, false), ("qt", Volume, GAL / 4.0, false),
    ("uk_qt", Volume, 4.546_09 / 4.0, false), ("gal", Volume, GAL, false),
    ("uk_gal", Volume, 4.546_09, false),
    ("l", Volume, 1.0, true), ("L", Volume, 1.0, true), ("lt", Volume, 1.0, true),
    ("ang3", Volume, 1e-27, true), ("barrel", Volume, 42.0 * GAL, false),
    ("bushel", Volume, 35.239_070_166_88, false), ("ft3", Volume, FT * FT * FT * 1000.0, false),
    ("in3", Volume, IN * IN * IN * 1000.0, false), ("ly3", Volume, LY * LY * LY * 1000.0, false),
    ("m3", Volume, 1000.0, true), ("mi3", Volume, MI * MI * MI * 1000.0, false),
    ("yd3", Volume, 27.0 * FT * FT * FT * 1000.0, false),
    ("Nmi3", Volume, NMI * NMI * NMI * 1000.0, false),
    ("Picapt3", Volume, PICA_PT * PICA_PT * PICA_PT * 1000.0, false),
    ("Pica3", Volume, PICA_PT * PICA_PT * PICA_PT * 1000.0, false),
    ("GRT", Volume, 100.0 * FT * FT * FT * 1000.0, false),
    ("regton", Volume, 100.0 * FT * FT * FT * 1000.0, false),
    ("MTON", Volume, 40.0 * FT * FT * FT * 1000.0, false),
    ("uk_acre", Area, 4_046.856_422_4, false),
    ("us_acre", Area, 4046.0 + 13_525_426.0 / 15_499_969.0, false), ("ang2", Area, 1e-20, true),
    ("ar", Area, 100.0, true), ("ha", Area, 10_000.0, false), ("ft2", Area, FT * FT, false),
    ("in2", Area, IN * IN, false), ("ly2", Area, LY * LY, false), ("m2", Area, 1.0, true),
    ("Morgen", Area, 2500.0, false), ("mi2", Area, MI * MI, false), ("Nmi2", Area, NMI * NMI, false),
    ("Picapt2", Area, PICA_PT * PICA_PT, false), ("Pica2", Area, PICA_PT * PICA_PT, false),
    ("yd2", Area, 9.0 * FT * FT, false),
    ("bit", Information, 1.0, true), ("byte", Information, 8.0, true),
    ("admkn", Speed, 1853.184 / 3600.0, false), ("kn", Speed, NMI / 3600.0, false),
    ("m/h", Speed, 1.0 / 3600.0, true), ("m/hr", Speed, 1.0 / 3600.0, true),
    ("m/s", Speed, 1.0, true), ("m/sec", Speed, 1.0, true), ("mph", Speed, MI / 3600.0, false),
];

/// An SI prefix's power of ten.
fn si(p: &str) -> Option<i32> {
    Some(match p {
        "Y" => 24,
        "Z" => 21,
        "E" => 18,
        "P" => 15,
        "T" => 12,
        "G" => 9,
        "M" => 6,
        "k" => 3,
        "h" => 2,
        "da" | "e" => 1,
        "d" => -1,
        "c" => -2,
        "m" => -3,
        "u" => -6,
        "n" => -9,
        "p" => -12,
        "f" => -15,
        "a" => -18,
        "z" => -21,
        "y" => -24,
        _ => return None,
    })
}

/// A binary prefix's power of two.
fn binary(p: &str) -> Option<i32> {
    Some(match p {
        "ki" => 10,
        "Mi" => 20,
        "Gi" => 30,
        "Ti" => 40,
        "Pi" => 50,
        "Ei" => 60,
        "Zi" => 70,
        "Yi" => 80,
        _ => return None,
    })
}

/// A unit spelling as (name, class, size, multiplier): an exact name, or a
/// prefix (squared or cubed with an area or volume unit) and a prefixable
/// name. `"m^2"` is `"m2"`.
fn unit(spelling: &str) -> Option<(&'static str, Class, f64, f64)> {
    let s = match spelling.char_indices().rev().nth(1) {
        Some((i, '^')) if i > 0 => format!("{}{}", &spelling[..i], &spelling[i + 1..]),
        _ => spelling.to_string(),
    };
    let find = |name: &str| UNITS.iter().find(|u| u.0 == name);
    if let Some(&(name, class, size, _)) = find(&s) {
        return Some((name, class, size, 1.0));
    }
    for len in [2, 1] {
        let Some((p, rest)) = s.split_at_checked(len) else {
            continue;
        };
        let Some(&(name, class, size, true)) = find(rest) else {
            continue;
        };
        let power = if rest.ends_with('2') {
            2
        } else if rest.ends_with('3') {
            3
        } else {
            1
        };
        if let Some(e) = si(p).filter(|_| len == 1 || p == "da") {
            return Some((name, class, size, 10f64.powi(e * power)));
        }
        if let Some(e) = binary(p).filter(|_| class == Information) {
            return Some((name, class, size, 2f64.powi(e)));
        }
    }
    None
}

/// A temperature in kelvin, from `x` of unit `name`.
fn to_kelvin(name: &str, x: f64) -> f64 {
    match name {
        "C" | "cel" => x + 273.15,
        "F" | "fah" => (x - 32.0) * 5.0 / 9.0 + 273.15,
        "Rank" => x * 5.0 / 9.0,
        "Reau" => x * 1.25 + 273.15,
        _ => x,
    }
}

/// A temperature in kelvin, as unit `name`.
fn from_kelvin(name: &str, k: f64) -> f64 {
    match name {
        "C" | "cel" => k - 273.15,
        "F" | "fah" => (k - 273.15) * 9.0 / 5.0 + 32.0,
        "Rank" => k * 9.0 / 5.0,
        "Reau" => (k - 273.15) * 0.8,
        _ => k,
    }
}

/// CONVERT(x, from, to): `#N/A` for an unknown unit or different classes.
pub(super) fn eval(a: &[CellValue]) -> R {
    let x = n(a, 0)?;
    let (from, to) = (unit(&t(a, 1)?), unit(&t(a, 2)?));
    let (Some((fname, fc, fsize, fmul)), Some((tname, tc, tsize, tmul))) = (from, to) else {
        return Err(err::NA);
    };
    if fc != tc {
        return Err(err::NA);
    }
    if fc == Temperature {
        if fname == tname && fmul == tmul {
            return ok_num(x);
        }
        return ok_num(from_kelvin(tname, to_kelvin(fname, x * fmul)) / tmul);
    }
    ok_num(x * fsize * fmul / (tsize * tmul))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn conv(x: f64, f: &str, to: &str) -> Result<f64, u8> {
        let a = [
            CellValue::num(x),
            CellValue::Text(f.into()),
            CellValue::Text(to.into()),
        ];
        eval(&a).map(|v| v.as_num().unwrap())
    }

    #[test]
    fn exact_definitions() {
        assert_eq!(conv(1.0, "lbm", "kg"), Ok(0.453_592_37));
        assert_eq!(conv(1.0, "in", "cm"), Ok(2.54));
        assert_eq!(conv(1.0, "gal", "l"), Ok(GAL));
        assert_eq!(conv(1.0, "kibyte", "byte"), Ok(1024.0));
        assert_eq!(conv(1.0, "Mibit", "kibit"), Ok(1024.0));
        assert_eq!(conv(1.0, "cm^3", "m3"), Ok(1e-6));
        assert_eq!(conv(1.0, "dam", "m"), Ok(10.0));
        assert_eq!(conv(5.0, "C", "C"), Ok(5.0));
        assert_eq!(conv(1.0, "mK", "K"), Ok(0.001));
        assert!((conv(0.0, "K", "Reau").unwrap() + 218.52).abs() < 1e-9);
        assert!((conv(0.0, "Rank", "C").unwrap() + 273.15).abs() < 1e-9);
    }

    #[test]
    fn unknown_and_mixed_units() {
        assert_eq!(conv(1.0, "m", "g"), Err(err::NA));
        assert_eq!(conv(1.0, "xyz", "m"), Err(err::NA));
        assert_eq!(conv(1.0, "kmi", "m"), Err(err::NA), "mi takes no prefix");
        assert_eq!(
            conv(1.0, "kim", "m"),
            Err(err::NA),
            "binary prefixes are for bits"
        );
        assert_eq!(conv(1.0, "dbyte", "^"), Err(err::NA));
        assert_eq!(conv(1.0, "", "m"), Err(err::NA));
        assert_eq!(conv(1.0, "é", "m"), Err(err::NA));
    }
}
