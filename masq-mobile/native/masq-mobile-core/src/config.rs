use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MobileConfig {
    pub chain: Chain,
    #[serde(rename = "rpcUrl")]
    pub rpc_url: String,
    pub neighbors: Vec<String>,
    #[serde(default = "default_min_hops", rename = "minHops")]
    pub min_hops: u8,
    #[serde(default, rename = "exitCountry")]
    pub exit_country: Option<String>,
    #[serde(
        default = "default_exit_country_fallback",
        rename = "exitCountryFallback"
    )]
    pub exit_country_fallback: bool,
    #[serde(default, rename = "dataDirectory")]
    pub data_directory: Option<String>,
}

fn default_min_hops() -> u8 {
    1
}

fn default_exit_country_fallback() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Chain {
    #[serde(rename = "base-mainnet")]
    BaseMainnet,
    #[serde(rename = "base-sepolia")]
    BaseSepolia,
}

impl Chain {
    pub fn identifier(self) -> &'static str {
        match self {
            Self::BaseMainnet => "base-mainnet",
            Self::BaseSepolia => "base-sepolia",
        }
    }
}

impl MobileConfig {
    pub fn parse(json: &str) -> Result<Self, String> {
        let config: Self =
            serde_json::from_str(json).map_err(|_| "Invalid mobile configuration.".to_owned())?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if !self.rpc_url.starts_with("https://") || self.rpc_url.len() <= "https://".len() {
            return Err("The blockchain RPC must use HTTPS.".to_owned());
        }
        if self.rpc_url.contains('@') {
            return Err("Do not include credentials in the RPC URL.".to_owned());
        }
        if self.neighbors.is_empty() {
            return Err("At least one entry node is required.".to_owned());
        }
        if !(1..=6).contains(&self.min_hops) {
            return Err("The MASQ route must use between one and six hops.".to_owned());
        }
        if let Some(country) = self.exit_country.as_deref() {
            if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
                return Err("The exit country must be a two-letter ISO country code.".to_owned());
            }
        }
        let prefix = format!("masq://{}:", self.chain.identifier());
        if self
            .neighbors
            .iter()
            .any(|descriptor| !descriptor.starts_with(&prefix) || !has_valid_port(descriptor))
        {
            return Err("An entry node does not match the selected chain.".to_owned());
        }
        if let Some(data_directory) = self.data_directory.as_deref() {
            if data_directory.is_empty() || !std::path::Path::new(data_directory).is_absolute() {
                return Err("The native data directory must be an absolute path.".to_owned());
            }
        }
        Ok(())
    }
}

fn has_valid_port(descriptor: &str) -> bool {
    descriptor
        .rsplit_once(':')
        .map(|(_, ports)| {
            ports
                .split('/')
                .all(|port| port.parse::<u16>().map_or(false, |port| port > 0))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_consume_configuration_for_the_matching_chain() {
        let json = r#"{
            "chain":"base-mainnet",
            "rpcUrl":"https://rpc.example/v1/key",
            "neighbors":["masq://base-mainnet:key@example.org:4433"]
        }"#;
        let config = MobileConfig::parse(json).unwrap();
        assert_eq!(config.chain, Chain::BaseMainnet);
        assert_eq!(config.min_hops, 1);
        assert_eq!(config.exit_country, None);
        assert!(config.exit_country_fallback);
    }

    #[test]
    fn rejects_a_neighbor_from_another_chain() {
        let json = r#"{
            "chain":"base-mainnet",
            "rpcUrl":"https://rpc.example",
            "neighbors":["masq://base-sepolia:key@example.org:4433"]
        }"#;
        assert!(MobileConfig::parse(json).is_err());
    }

    #[test]
    fn accepts_an_entry_descriptor_with_multiple_ports() {
        let json = r#"{
            "chain":"base-sepolia",
            "rpcUrl":"https://rpc.example",
            "neighbors":["masq://base-sepolia:key@example.org:4433/4434"]
        }"#;
        assert!(MobileConfig::parse(json).is_ok());
    }

    #[test]
    fn accepts_route_and_exit_country_preferences() {
        let json = r#"{
            "chain":"base-mainnet",
            "rpcUrl":"https://rpc.example",
            "neighbors":["masq://base-mainnet:key@example.org:4433"],
            "minHops":3,
            "exitCountry":"BE",
            "exitCountryFallback":false
        }"#;
        let config = MobileConfig::parse(json).unwrap();
        assert_eq!(config.min_hops, 3);
        assert_eq!(config.exit_country.as_deref(), Some("BE"));
        assert!(!config.exit_country_fallback);
    }

    #[test]
    fn rejects_invalid_route_preferences() {
        let invalid_hops = r#"{
            "chain":"base-mainnet",
            "rpcUrl":"https://rpc.example",
            "neighbors":["masq://base-mainnet:key@example.org:4433"],
            "minHops":7
        }"#;
        let invalid_country = r#"{
            "chain":"base-mainnet",
            "rpcUrl":"https://rpc.example",
            "neighbors":["masq://base-mainnet:key@example.org:4433"],
            "exitCountry":"Belgium"
        }"#;
        assert!(MobileConfig::parse(invalid_hops).is_err());
        assert!(MobileConfig::parse(invalid_country).is_err());
    }
}
