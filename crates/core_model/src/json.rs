//! JSON for models and formulas, at any depth a formula may have.
//!
//! serde_json refuses input nested more than 128 arrays and objects deep: a
//! formula 40 levels deep is already 126 of them in an exported model. These
//! read and write past that. The reader first counts the nesting, without
//! recursing, against [`MAX_DEPTH`]; both run on a stack that grows
//! (`serde_stacker`), so a deep formula does not overflow a small thread.

use serde::{de::DeserializeOwned, Serialize};

/// The deepest JSON [`from_str`] reads: a whole model (5 levels) holding a
/// formula (1) [`crate::MAX_STORED_FORMULA_DEPTH`] deep (3 levels per tree
/// level, 1 more for its leaf), with room.
pub const MAX_DEPTH: usize = 3 * crate::MAX_STORED_FORMULA_DEPTH + 16;

/// How the error for JSON past [`MAX_DEPTH`] starts: serde_json's words for
/// its own limit, which [`crate::Formula::from_json`] tells apart.
pub(crate) const TOO_DEEP: &str = "recursion limit exceeded";

/// `serde_json::from_str`, to [`MAX_DEPTH`] levels instead of 128.
pub fn from_str<T: DeserializeOwned>(json: &str) -> serde_json::Result<T> {
    from_slice(json.as_bytes())
}

/// `serde_json::from_slice`, to [`MAX_DEPTH`] levels instead of 128.
pub fn from_slice<T: DeserializeOwned>(json: &[u8]) -> serde_json::Result<T> {
    if nesting(json) > MAX_DEPTH {
        return Err(<serde_json::Error as serde::de::Error>::custom(format!(
            "{TOO_DEEP}: the JSON nests more than {MAX_DEPTH} levels"
        )));
    }
    let mut de = serde_json::Deserializer::from_slice(json);
    de.disable_recursion_limit();
    let v = T::deserialize(serde_stacker::Deserializer::new(&mut de))?;
    de.end()?;
    Ok(v)
}

/// `serde_json::to_string` on a stack that grows.
pub fn to_string<T: Serialize + ?Sized>(v: &T) -> serde_json::Result<String> {
    let mut out = Vec::new();
    v.serialize(serde_stacker::Serializer::new(
        &mut serde_json::Serializer::new(&mut out),
    ))?;
    Ok(String::from_utf8(out).expect("serde_json writes UTF-8"))
}

/// `serde_json::to_string_pretty` on a stack that grows.
pub fn to_string_pretty<T: Serialize + ?Sized>(v: &T) -> serde_json::Result<String> {
    let mut out = Vec::new();
    v.serialize(serde_stacker::Serializer::new(
        &mut serde_json::Serializer::pretty(&mut out),
    ))?;
    Ok(String::from_utf8(out).expect("serde_json writes UTF-8"))
}

/// How deeply `json` nests arrays and objects, strings skipped, counted in a
/// loop; it stops counting once past [`MAX_DEPTH`].
fn nesting(json: &[u8]) -> usize {
    let (mut depth, mut max, mut in_str, mut escaped) = (0usize, 0usize, false, false);
    for &b in json {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
                if max > MAX_DEPTH {
                    break;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn nesting_skips_strings_and_escapes() {
        assert_eq!(nesting(b"1"), 0);
        assert_eq!(nesting(br#"{"a":"[[\"]]{","b":[[1],[]]}"#), 3);
        assert_eq!(nesting(b"]]}["), 1);
    }

    #[test]
    fn reads_and_writes_past_serde_jsons_limit() {
        let deep = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        let v: serde_json::Value = from_str(&deep(MAX_DEPTH)).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&deep(MAX_DEPTH)).is_err());
        assert_eq!(to_string(&v).unwrap(), deep(MAX_DEPTH));
        assert!(to_string_pretty(&v).unwrap().starts_with("[\n  ["));
        // `Value`'s drop recurses; this one is shallow enough.
        drop(v);
        let e = from_str::<serde_json::Value>(&deep(MAX_DEPTH + 1)).unwrap_err();
        assert!(e.to_string().starts_with(TOO_DEEP), "{e}");
        // A huge input stops counting early, and is never parsed.
        let e = from_slice::<serde_json::Value>(deep(1 << 20).as_bytes()).unwrap_err();
        assert!(e.to_string().starts_with(TOO_DEEP), "{e}");
        // Trailing junk and bad JSON are errors, as in serde_json.
        assert!(from_str::<u32>("1 2").is_err());
        assert!(from_str::<u32>("[").is_err());
    }
}
