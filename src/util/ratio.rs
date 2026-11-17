//! Rational numbers for frame rates and time bases.

use std::fmt;
use std::str::FromStr;

/// A positive rational number such as `30000/1001`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ratio {
    pub num: u32,
    pub den: u32,
}

impl Ratio {
    pub const fn new(num: u32, den: u32) -> Ratio {
        Ratio { num, den }
    }

    pub const fn integer(n: u32) -> Ratio {
        Ratio { num: n, den: 1 }
    }

    pub fn as_f64(self) -> f64 {
        if self.den == 0 {
            0.0
        } else {
            self.num as f64 / self.den as f64
        }
    }

    /// Rounded up to the next integer (for GOP sizes and the like).
    pub fn ceil(self) -> u32 {
        if self.den == 0 {
            0
        } else {
            self.num.div_ceil(self.den)
        }
    }
}

impl fmt::Display for Ratio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl FromStr for Ratio {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Ratio, Self::Err> {
        let (num, den) = match s.split_once('/') {
            Some((n, d)) => (n, d),
            None => (s, "1"),
        };
        let num = num.trim().parse().map_err(|_| "invalid ratio numerator")?;
        let den: u32 = den
            .trim()
            .parse()
            .map_err(|_| "invalid ratio denominator")?;
        if num == 0 || den == 0 {
            return Err("ratio must be positive");
        }
        Ok(Ratio { num, den })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let r: Ratio = "30000/1001".parse().unwrap();
        assert_eq!(r.to_string(), "30000/1001");
        assert_eq!(r.ceil(), 30);
        assert_eq!("60".parse::<Ratio>().unwrap(), Ratio::integer(60));
        assert!("0".parse::<Ratio>().is_err());
    }
}
