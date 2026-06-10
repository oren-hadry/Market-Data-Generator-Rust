use std::collections::HashMap;
use std::fs;

use crate::types::{Config, RateProfile, Symbol};

pub fn parse_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot open config {path}: {e}"))?;

    let root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("JSON parse error: {e}"))?;

    let data_mode = root["data_mode"].as_str().unwrap_or("synthetic").to_string();
    let base_rate = root["base_rate"].as_f64().unwrap_or(1000.0);
    let max_rate = root["max_rate"].as_f64().unwrap_or(5000.0);

    let rate_profile = match root["rate_profile"].as_str().unwrap_or("sine") {
        "sine"     => RateProfile::SineWave,
        "constant" => RateProfile::Constant,
        "burst"    => RateProfile::Burst,
        "ramp"     => RateProfile::Ramp,
        rp         => return Err(format!("unknown rate_profile: {rp}").into()),
    };

    let symbols: Vec<Symbol> = root["symbols"]
        .as_array()
        .ok_or("config missing symbols array")?
        .iter()
        .map(|v| Symbol::from_str(v.as_str().unwrap_or("")).map_err(|e| e.into()))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

    if symbols.is_empty() {
        return Err("config missing symbols".into());
    }

    let mut initial_mid_prices = HashMap::new();
    for &sym in &symbols {
        let price = root["initial_mid_prices"][sym.as_str()]
            .as_f64()
            .ok_or_else(|| format!("config missing initial_mid_prices.{}", sym.as_str()))?;
        initial_mid_prices.insert(sym, price);
    }

    Ok(Config { rate_profile, base_rate, max_rate, data_mode, symbols, initial_mid_prices })
}
