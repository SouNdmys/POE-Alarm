//! Editor validation is deliberately separate from rule execution.
use poe_alarm_core::{Decimal, NumericConstraint, NumericConstraintMode as M};

pub fn parse_constraint(
    mode: M,
    minimum: &str,
    maximum: &str,
) -> Result<NumericConstraint, &'static str> {
    let number = |text: &str| text.trim().parse::<Decimal>().map_err(|_| "invalid_number");
    Ok(match mode {
        M::Ignore => NumericConstraint::default(),
        M::AtLeast => NumericConstraint {
            mode,
            minimum: Some(number(minimum)?),
            ..Default::default()
        },
        M::AtMost => NumericConstraint {
            mode,
            maximum: Some(number(maximum)?),
            ..Default::default()
        },
        M::Exactly => NumericConstraint {
            mode,
            expected: Some(number(minimum)?),
            ..Default::default()
        },
        M::RangeInclusive => {
            let minimum = number(minimum)?;
            let maximum = number(maximum)?;
            if minimum > maximum {
                return Err("reversed_range");
            }
            NumericConstraint {
                mode,
                minimum: Some(minimum),
                maximum: Some(maximum),
                ..Default::default()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_comparisons_never_silently_become_unrestricted() {
        for bad in ["", "7O", "NaN", "inf", "1e100", "--3"] {
            assert!(parse_constraint(M::AtLeast, bad, "").is_err(), "{bad}");
            assert!(parse_constraint(M::Exactly, bad, "").is_err(), "{bad}");
            assert!(parse_constraint(M::AtMost, "", bad).is_err(), "{bad}");
        }
        assert!(parse_constraint(M::RangeInclusive, "200", "100").is_err());
        assert!(parse_constraint(M::RangeInclusive, "100", "").is_err());
        assert_eq!(
            parse_constraint(M::Ignore, "7O", "bad").unwrap(),
            NumericConstraint::default()
        );
    }
    #[test]
    fn decimals_and_cross_zero_bounds_remain_exact() {
        let range = parse_constraint(M::RangeInclusive, " -0.01 ", "0.02").unwrap();
        assert_eq!(range.minimum.unwrap().to_string(), "-0.01");
        assert_eq!(range.maximum.unwrap().to_string(), "0.02");
    }
}
