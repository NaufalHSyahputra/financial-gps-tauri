use super::{AppError, Result};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrencyCode(String);

impl CurrencyCode {
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 3 || !value.bytes().all(|byte| byte.is_ascii_uppercase()) {
            return Err(AppError::validation("currency", "invalid_currency"));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrencyScale {
    Zero,
    Two,
    Three,
}

impl CurrencyScale {
    fn digits(self) -> u32 {
        match self {
            Self::Zero => 0,
            Self::Two => 2,
            Self::Three => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Money {
    pub currency: CurrencyCode,
    pub minor: i64,
}

impl Money {
    pub fn from_major(currency: CurrencyCode, scale: CurrencyScale, value: &str) -> Result<Self> {
        let invalid = || AppError::validation("amount", "invalid_exact_amount");
        if value.is_empty()
            || !value.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_digit() || byte == b'.' || (index == 0 && byte == b'-')
            })
        {
            return Err(invalid());
        }
        let amount = Decimal::from_str_exact(value).map_err(|_| invalid())?;
        let minor = amount
            .checked_mul(Decimal::from(10_i64.pow(scale.digits())))
            .filter(Decimal::is_integer)
            .and_then(|amount| amount.to_i64())
            .ok_or_else(invalid)?;
        Ok(Self { currency, minor })
    }

    pub fn to_major(&self, scale: CurrencyScale) -> Decimal {
        Decimal::new(self.minor, scale.digits())
    }
}
