//! Money helpers. Every statutory amount in this crate is computed with
//! `rust_decimal` — never `f64` — because EPF/EPS splits and TDS rounding are
//! audited to the paisa and then to the rupee.

use rust_decimal::{Decimal, RoundingStrategy};

/// Convert a configuration float into a decimal.
///
/// Configuration rates are written as decimals (`0.0833`), so the shortest
/// round-trip representation of the `f64` is the exact statutory rate; going
/// through `from_f64_retain` would keep the binary noise and turn
/// `15000 * 0.0833` into `1249.4999…`, breaking the rupee rounding rule.
pub fn dec(value: f64) -> Decimal {
    use std::str::FromStr;
    Decimal::from_str(&value.to_string())
        .or_else(|_| Decimal::from_str_exact(&value.to_string()))
        .unwrap_or_else(|_| Decimal::from_f64_retain(value).unwrap_or(Decimal::ZERO))
}

/// Money is carried at paise precision internally and rounded half-away-from
/// zero (the convention used by Indian payroll software).
pub fn round2(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// Round to whole rupees (TDS, EPF, PT).
pub fn round_rupee(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
}

/// Round *up* to the next whole rupee (ESI: "fractions to be rounded up").
pub fn round_rupee_up(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(0, RoundingStrategy::ToPositiveInfinity)
}

pub fn max(a: Decimal, b: Decimal) -> Decimal {
    if a > b {
        a
    } else {
        b
    }
}

pub fn min(a: Decimal, b: Decimal) -> Decimal {
    if a < b {
        a
    } else {
        b
    }
}

pub fn zero() -> Decimal {
    Decimal::ZERO
}

/// `value * rate`, rounded to paise.
pub fn pct(value: Decimal, rate: Decimal) -> Decimal {
    round2(value * rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn config_rates_convert_exactly() {
        assert_eq!(dec(0.0833) * Decimal::from(15000), Decimal::from_str("1249.5").unwrap());
        assert_eq!(dec(0.0325) * Decimal::from(24000), Decimal::from(780));
        assert_eq!(dec(0.0075) * Decimal::from(21000), Decimal::from_str("157.5").unwrap());
    }

    #[test]
    fn rupee_rounding_conventions() {
        assert_eq!(round_rupee(Decimal::from_str("1249.50").unwrap()), Decimal::from(1250));
        assert_eq!(round_rupee(Decimal::from_str("1249.49").unwrap()), Decimal::from(1249));
        assert_eq!(round_rupee_up(Decimal::from_str("157.01").unwrap()), Decimal::from(158));
        assert_eq!(round_rupee_up(Decimal::from(157)), Decimal::from(157));
        assert_eq!(round2(Decimal::from_str("173076.923").unwrap()), Decimal::from_str("173076.92").unwrap());
    }
}
