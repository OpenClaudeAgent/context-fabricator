use super::AnswerLanguage;

pub(super) fn with_verified_unit<'a>(
    answer: &str,
    language: AnswerLanguage,
    rust_quotes: impl IntoIterator<Item = &'a str>,
) -> String {
    let answer = answer.trim();
    let (label, quantity) = answer
        .split_once(':')
        .map_or((None, answer), |(name, tail)| {
            (Some(name.trim()), tail.trim())
        });
    let (number, provided) = quantity
        .split_once(char::is_whitespace)
        .map_or((quantity, None), |(number, unit)| {
            (number, Some(unit.trim()))
        });
    let Some(value) = decimal(number) else {
        return answer.to_owned();
    };
    let mut matching = rust_quotes
        .into_iter()
        // A preceding line can be added to the displayed quote for context.
        .filter_map(|quote| quote.lines().last())
        .filter_map(integer_constant)
        .filter(|(name, constant)| *constant == value && label.is_none_or(|label| label == *name))
        .map(|(name, _)| unit(name));
    // Agree on both the literal and the unit across matching cited declarations.
    if let Some(Some(unit)) = matching.next()
        && matching.all(|other| other == Some(unit))
        && provided.is_none_or(|supplied| language == AnswerLanguage::Fr && supplied == unit)
    {
        if language == AnswerLanguage::Fr {
            return format!("{value} {}", french_unit(unit, value));
        }
        return format!("{value} {unit}");
    }
    answer.to_owned()
}

fn french_unit(symbol: &str, value: i128) -> &str {
    match (symbol, value.unsigned_abs() == 1) {
        ("ms", true) => "milliseconde",
        ("ms", false) => "millisecondes",
        ("s", true) => "seconde",
        ("s", false) => "secondes",
        ("B", true) => "octet",
        ("B", false) => "octets",
        ("req/min", true) => "requête par minute",
        ("req/min", false) => "requêtes par minute",
        _ => symbol,
    }
}

pub(super) fn integer_constant(line: &str) -> Option<(&str, i128)> {
    let (visibility, constant) = line.trim().split_once("const ")?;
    if !matches!(visibility, "" | "pub " | "pub(crate) ") {
        return None;
    }
    let (declaration, literal) = constant.strip_suffix(';')?.split_once('=')?;
    let (name, integer_type) = declaration.split_once(':')?;
    let name = name.trim();
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        || !matches!(
            integer_type.trim(),
            "i8" | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
        )
    {
        return None;
    }
    Some((name, decimal(literal.trim())?))
}

fn decimal(text: &str) -> Option<i128> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !digits.as_bytes().first()?.is_ascii_digit()
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    text.replace('_', "").parse().ok()
}

fn unit(name: &str) -> Option<&'static str> {
    if name.ends_with("_REQUESTS_PER_MINUTE") {
        return Some("req/min");
    }
    match name.rsplit_once('_')?.1 {
        "BYTES" => Some("B"),
        "MS" => Some("ms"),
        "SECONDS" => Some("s"),
        _ => None,
    }
}

/// A linked-code draft must state the value of its cited declaration.
pub(super) fn matches_declaration(answer: &str, declaration: &str) -> bool {
    let Some((name, exact)) = integer_constant(declaration) else {
        return false;
    };
    let answer = answer.trim().trim_end_matches('.');
    let (number, reported) = answer
        .split_once(char::is_whitespace)
        .map_or((answer, None), |(number, unit)| (number, Some(unit.trim())));
    let Some(value) = decimal(number) else {
        return false;
    };
    let Some(implied) = unit(name) else {
        return false;
    };
    let stated = match reported {
        None => return value == exact,
        Some(
            "ms" | "millisecond" | "milliseconds" | "milliseconde" | "millisecondes"
            | "Millisekunden",
        ) => "ms",
        Some("s" | "second" | "seconds" | "seconde" | "secondes" | "Sekunde" | "Sekunden") => "s",
        Some("B" | "byte" | "bytes" | "octet" | "octets") => "B",
        Some("req/min" | "requests per minute" | "requêtes par minute") => "req/min",
        _ => return false,
    };
    if (stated, implied) == ("s", "ms") {
        value.checked_mul(1_000) == Some(exact)
    } else {
        stated == implied && value == exact
    }
}

#[cfg(test)]
mod tests {
    use super::{AnswerLanguage, matches_declaration, with_verified_unit};

    #[test]
    fn linked_drafts_match_a_literal_and_its_dimension() {
        let declaration = "pub const INTERVAL_MS: i64 = 1_000;";
        for answer in [
            "1000 millisecondes",
            "1 seconde",
            "1 second",
            "1 Sekunde",
            "1_000",
        ] {
            assert!(matches_declaration(answer, declaration), "{answer}");
        }
        for answer in [
            "2 secondes",
            "1000 seconds",
            "1000 bytes",
            "1 s or 2 s",
            "1",
        ] {
            assert!(!matches_declaration(answer, declaration), "{answer}");
        }
        assert!(matches_declaration(
            "80 octets",
            "const LIMIT_BYTES: usize = 80;"
        ));
        assert!(!matches_declaration("12", "const LIMIT: usize = 3 * 4;"));
        assert!(!matches_declaration("10", "const RETRY_MINUTES: u64 = 10;"));
    }

    #[test]
    fn reads_the_units_of_cited_integer_constants() {
        for (answer, quote, expected) in [
            (
                "60",
                "pub const MAX_REQUESTS_PER_MINUTE: u16 = 60;",
                "60 req/min",
            ),
            (
                "1_000",
                "pub const UNCERTAIN_COOLDOWN_MS: i64 = 1_000;",
                "1000 ms",
            ),
            (
                "UNCERTAIN_COOLDOWN_MS: 1_000",
                "pub const UNCERTAIN_COOLDOWN_MS: i64 = 1_000;",
                "1000 ms",
            ),
            (
                "3072",
                "pub const MAX_PASSAGE_BYTES: usize = 3072;",
                "3072 B",
            ),
            ("-5", "const OFFSET_SECONDS: i32 = -5;", "-5 s"),
        ] {
            assert_eq!(
                with_verified_unit(answer, AnswerLanguage::Auto, [quote]),
                expected
            );
        }
    }

    #[test]
    fn keeps_prose_and_unconfirmed_quantities_as_written() {
        let milliseconds = "const RETRY_MS: u32 = 10;";
        for answer in [
            "10 milliseconds",
            "10 minutes",
            "100",
            "10.0",
            "10, 20",
            "OTHER_MS: 10",
        ] {
            assert_eq!(
                with_verified_unit(answer, AnswerLanguage::Auto, [milliseconds]),
                answer
            );
        }
        for another in [
            "const RETRY_SECONDS: u32 = 10;",
            "const ATTEMPTS: u32 = 10;",
        ] {
            assert_eq!(
                with_verified_unit("10", AnswerLanguage::Auto, [milliseconds, another]),
                "10"
            );
        }
        for quote in [
            "const RETRY_MS: u32 = 10;\nassert_eq!(attempts, 10);",
            "// const RETRY_MS: u32 = 10;",
            "const RETRY_MS: u32 = 5 * 2;",
            "const RETRY_MS: u32 = 10",
            "const ATTEMPTS: u32 = 10;",
        ] {
            assert_eq!(
                with_verified_unit("10", AnswerLanguage::Auto, [quote]),
                "10"
            );
        }
    }

    #[test]
    fn french_formats_a_verified_short_quantity_without_changing_its_value() {
        let quote = "const RETRY_MS: u32 = 1_000;";
        assert_eq!(
            with_verified_unit("1_000 ms", AnswerLanguage::Fr, [quote]),
            "1000 millisecondes"
        );
        assert_eq!(
            with_verified_unit("1000 s", AnswerLanguage::Fr, [quote]),
            "1000 s"
        );
        assert_eq!(
            with_verified_unit("1000 ms", AnswerLanguage::En, [quote]),
            "1000 ms"
        );
    }
}
