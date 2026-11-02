//! Cell entry: what the text typed into a cell means, by the measure's
//! declared value type. The same rules as `crates/gui` (`parse_typed`):
//! the declared type wins over the text's shape, so `42` typed into a Text
//! measure is text, and text that does not parse as the type is refused.
//! An empty entry clears the cell.

use improv_core_model::{parser, Expr, Locale, Model, Value, ValueType};

/// The value `text` sets a cell of type `ty` to: `Ok(None)` clears it.
pub fn parse(
    model: &Model,
    locale: &Locale,
    ty: ValueType,
    text: &str,
) -> Result<Option<Value>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let literal = |src: String| match parser::parse_expr(model, &src) {
        Ok(f) => match f.expr {
            Expr::Literal(v) if v.type_of() == Some(ty) => Some(v),
            _ => None,
        },
        Err(_) => None,
    };
    let v = match ty {
        ValueType::Number => locale
            .parse_number(text)
            .map(Value::Number)
            .ok_or_else(|| format!("not a number: {text:?} ({})", locale.number_hint()))?,
        ValueType::Text => Value::Text(text.to_string()),
        ValueType::Boolean => literal(text.to_ascii_uppercase())
            .ok_or_else(|| format!("not a boolean (use true or false): {text:?}"))?,
        ValueType::DateTime => literal(format!("#{text}#"))
            .ok_or_else(|| format!("not a date (use YYYY-MM-DD or RFC 3339): {text:?}"))?,
        ValueType::Enum => text
            .parse::<u32>()
            .map(Value::Enum)
            .map_err(|_| format!("not an enum index: {text:?}"))?,
    };
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(ty: ValueType, t: &str) -> Result<Option<Value>, String> {
        parse(&Model::new(), &Locale::en(), ty, t)
    }

    #[test]
    fn the_declared_type_decides() {
        assert_eq!(
            p(ValueType::Number, " 1,500.5 "),
            Ok(Some(Value::Number(1500.5)))
        );
        assert_eq!(p(ValueType::Text, "42"), Ok(Some(Value::Text("42".into()))));
        assert_eq!(
            p(ValueType::Boolean, "true"),
            Ok(Some(Value::Boolean(true)))
        );
        assert!(p(ValueType::Number, "abc")
            .unwrap_err()
            .contains("not a number"));
        assert!(p(ValueType::Boolean, "yes").is_err());
        assert!(matches!(
            p(ValueType::DateTime, "2026-11-02"),
            Ok(Some(Value::DateTime(_)))
        ));
        assert!(p(ValueType::DateTime, "soon").is_err());
        assert_eq!(p(ValueType::Enum, "3"), Ok(Some(Value::Enum(3))));
        assert!(p(ValueType::Enum, "-1").is_err());
    }

    #[test]
    fn an_empty_entry_clears() {
        assert_eq!(p(ValueType::Number, "   "), Ok(None));
    }

    #[test]
    fn numbers_follow_the_locale() {
        let de = Locale::de();
        let m = Model::new();
        assert_eq!(
            parse(&m, &de, ValueType::Number, "1,5"),
            Ok(Some(Value::Number(1.5)))
        );
    }
}
