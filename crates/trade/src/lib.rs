use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::fmt::Formatter;
use std::str::FromStr;

pub mod cfd;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ContractSymbol {
    BtcUsd,
}

impl ContractSymbol {
    pub fn label(self) -> String {
        match self {
            ContractSymbol::BtcUsd => "btcusd".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Direction {
    Long,
    Short,
}

impl FromStr for ContractSymbol {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_lowercase().as_str() {
            "btcusd" => Ok(ContractSymbol::BtcUsd),
            // BitMEX representation
            "xbtusd" => Ok(ContractSymbol::BtcUsd),
            unknown => bail!("Unknown contract symbol {unknown}"),
        }
    }
}

impl fmt::Display for ContractSymbol {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let symbol = match self {
            ContractSymbol::BtcUsd => "btcusd",
        };
        symbol.to_string().fmt(f)
    }
}

#[cfg(test)]
pub mod tests {
    use crate::ContractSymbol;
    use std::str::FromStr;

    #[test]
    pub fn contract_symbol_from_str() {
        assert_eq!(
            ContractSymbol::from_str("btcusd").unwrap(),
            ContractSymbol::BtcUsd
        );
        assert_eq!(
            ContractSymbol::from_str("BTCUSD").unwrap(),
            ContractSymbol::BtcUsd
        );
        assert_eq!(
            ContractSymbol::from_str("xbtusd").unwrap(),
            ContractSymbol::BtcUsd
        );
        assert!(ContractSymbol::from_str("dogeusd").is_err());
    }
}