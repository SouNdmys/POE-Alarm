use std::cmp::Ordering;
use std::fmt::{Debug, Display};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};

/// An exact base-10 value used for every displayed roll and rule boundary.
///
/// Values are normalized (`3.10 == 3.1`) and support up to 38 fractional
/// digits. JSON serialization remains a JSON number, not a quoted string.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct Decimal {
    coefficient: i128,
    scale: u32,
}

impl Decimal {
    pub const MAX_SCALE: u32 = 38;

    pub const ZERO: Self = Self {
        coefficient: 0,
        scale: 0,
    };

    pub fn from_parts(coefficient: i128, scale: u32) -> Result<Self, DecimalParseError> {
        if scale > Self::MAX_SCALE {
            return Err(DecimalParseError::ScaleTooLarge(scale));
        }
        Ok(Self::normalized(coefficient, scale))
    }

    pub fn coefficient(self) -> i128 {
        self.coefficient
    }

    pub fn scale(self) -> u32 {
        self.scale
    }

    fn normalized(mut coefficient: i128, mut scale: u32) -> Self {
        if coefficient == 0 {
            return Self::ZERO;
        }
        while scale > 0 && coefficient % 10 == 0 {
            coefficient /= 10;
            scale -= 1;
        }
        Self { coefficient, scale }
    }

    fn absolute_cmp(self, other: Self) -> Ordering {
        let left = self.coefficient.unsigned_abs();
        let right = other.coefficient.unsigned_abs();
        let left_factor = power_of_ten(self.scale);
        let right_factor = power_of_ten(other.scale);
        let left_integer = left / left_factor;
        let right_integer = right / right_factor;
        match left_integer.cmp(&right_integer) {
            Ordering::Equal => {
                let common_scale = self.scale.max(other.scale);
                let left_fraction = (left % left_factor) * power_of_ten(common_scale - self.scale);
                let right_fraction =
                    (right % right_factor) * power_of_ten(common_scale - other.scale);
                left_fraction.cmp(&right_fraction)
            }
            ordering => ordering,
        }
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.coefficient.signum(), other.coefficient.signum()) {
            (left, right) if left != right => left.cmp(&right),
            (-1, -1) => self.absolute_cmp(*other).reverse(),
            _ => self.absolute_cmp(*other),
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for Decimal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.scale == 0 {
            return write!(formatter, "{}", self.coefficient);
        }
        let negative = self.coefficient < 0;
        let digits = self.coefficient.unsigned_abs().to_string();
        if negative {
            formatter.write_str("-")?;
        }
        let scale = self.scale as usize;
        if digits.len() <= scale {
            formatter.write_str("0.")?;
            for _ in 0..scale - digits.len() {
                formatter.write_str("0")?;
            }
            formatter.write_str(&digits)
        } else {
            let split = digits.len() - scale;
            write!(formatter, "{}.{}", &digits[..split], &digits[split..])
        }
    }
}

impl Debug for Decimal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Decimal({self})")
    }
}

impl FromStr for Decimal {
    type Err = DecimalParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let source = value.trim();
        if source.is_empty() {
            return Err(DecimalParseError::InvalidSyntax(value.to_owned()));
        }

        let (mantissa, exponent) = match source.find(['e', 'E']) {
            Some(index) => {
                if source[index + 1..].contains(['e', 'E']) {
                    return Err(DecimalParseError::InvalidSyntax(value.to_owned()));
                }
                let exponent = source[index + 1..]
                    .parse::<i32>()
                    .map_err(|_| DecimalParseError::InvalidSyntax(value.to_owned()))?;
                (&source[..index], exponent)
            }
            None => (source, 0),
        };

        let (negative, unsigned) = match mantissa.as_bytes().first() {
            Some(b'-') => (true, &mantissa[1..]),
            Some(b'+') => (false, &mantissa[1..]),
            _ => (false, mantissa),
        };
        if unsigned.is_empty() {
            return Err(DecimalParseError::InvalidSyntax(value.to_owned()));
        }

        let mut digits = String::with_capacity(unsigned.len());
        let mut fractional_digits = 0i32;
        let mut saw_decimal = false;
        let mut digit_count = 0;
        for character in unsigned.chars() {
            match character {
                '0'..='9' => {
                    digits.push(character);
                    digit_count += 1;
                    if saw_decimal {
                        fractional_digits += 1;
                    }
                }
                '.' if !saw_decimal => saw_decimal = true,
                _ => return Err(DecimalParseError::InvalidSyntax(value.to_owned())),
            }
        }
        if digit_count == 0 {
            return Err(DecimalParseError::InvalidSyntax(value.to_owned()));
        }

        let signed_digits = if negative {
            format!("-{digits}")
        } else {
            digits
        };
        let mut coefficient = signed_digits
            .parse::<i128>()
            .map_err(|_| DecimalParseError::MagnitudeTooLarge)?;
        let resulting_scale = fractional_digits
            .checked_sub(exponent)
            .ok_or(DecimalParseError::MagnitudeTooLarge)?;
        let scale = if resulting_scale < 0 {
            let shift = resulting_scale.unsigned_abs();
            coefficient = coefficient
                .checked_mul(power_of_ten_checked(shift)?)
                .ok_or(DecimalParseError::MagnitudeTooLarge)?;
            0
        } else {
            resulting_scale as u32
        };
        Self::from_parts(coefficient, scale)
    }
}

impl Serialize for Decimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let number = serde_json::Number::from_str(&self.to_string()).map_err(ser::Error::custom)?;
        number.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let number = serde_json::Number::deserialize(deserializer)?;
        Self::from_str(number.as_str()).map_err(de::Error::custom)
    }
}

macro_rules! integer_conversions {
    ($($type:ty),* $(,)?) => {
        $(
            impl From<$type> for Decimal {
                fn from(value: $type) -> Self {
                    Self {
                        coefficient: value as i128,
                        scale: 0,
                    }
                }
            }
        )*
    };
}

integer_conversions!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl From<i128> for Decimal {
    fn from(value: i128) -> Self {
        Self {
            coefficient: value,
            scale: 0,
        }
    }
}

impl From<f64> for Decimal {
    fn from(value: f64) -> Self {
        assert!(value.is_finite(), "decimal input must be finite");
        value
            .to_string()
            .parse()
            .expect("a finite f64 always has a valid decimal rendering")
    }
}

impl From<f32> for Decimal {
    fn from(value: f32) -> Self {
        assert!(value.is_finite(), "decimal input must be finite");
        value
            .to_string()
            .parse()
            .expect("a finite f32 always has a valid decimal rendering")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecimalParseError {
    InvalidSyntax(String),
    ScaleTooLarge(u32),
    MagnitudeTooLarge,
}

impl Display for DecimalParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSyntax(value) => write!(formatter, "invalid decimal '{value}'"),
            Self::ScaleTooLarge(scale) => {
                write!(
                    formatter,
                    "decimal scale {scale} exceeds {}",
                    Decimal::MAX_SCALE
                )
            }
            Self::MagnitudeTooLarge => formatter.write_str("decimal magnitude is too large"),
        }
    }
}

impl std::error::Error for DecimalParseError {}

fn power_of_ten(scale: u32) -> u128 {
    10u128.pow(scale)
}

fn power_of_ten_checked(scale: u32) -> Result<i128, DecimalParseError> {
    if scale > Decimal::MAX_SCALE {
        return Err(DecimalParseError::ScaleTooLarge(scale));
    }
    10i128
        .checked_pow(scale)
        .ok_or(DecimalParseError::MagnitudeTooLarge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_and_ordering_are_exact() {
        let three_one: Decimal = "3.1".parse().unwrap();
        assert_eq!(three_one, "3.10".parse().unwrap());
        assert!("3.099999".parse::<Decimal>().unwrap() < three_one);
        assert!("-3.100001".parse::<Decimal>().unwrap() < "-3.1".parse().unwrap());
        assert_eq!("1e-3".parse::<Decimal>().unwrap().to_string(), "0.001");
    }

    #[test]
    fn json_round_trip_stays_a_number() {
        let value: Decimal = "3.1000000000000000000001".parse().unwrap();
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, "3.1000000000000000000001");
        assert_eq!(serde_json::from_str::<Decimal>(&json).unwrap(), value);
    }
}
