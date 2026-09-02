//! Floway gateway client: fetch the `/v1/models` payload that every harness
//! configuration is derived from.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone)]
pub struct Client {
    endpoint: String,
    api_key: String,
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn new(endpoint: String, api_key: String) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Self {
            endpoint,
            api_key,
            http,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// GET /v1/models with the key as a bearer token, mirroring the harness
    /// installers (`curl --oauth2-bearer`).
    pub fn fetch_models(&self) -> Result<ModelList> {
        let url = format!("{}/v1/models", self.endpoint);
        let response = self
            .http
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .with_context(|| format!("could not connect to {url}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            let body = if body.len() > 300 {
                body[..300].to_string()
            } else {
                body
            };
            anyhow::bail!("the gateway returned {status} for {url}: {body}");
        }
        let models: ModelList = response
            .json()
            .with_context(|| format!("{url} did not return a Floway model list"))?;
        Ok(models)
    }
}

#[derive(Debug, Deserialize)]
pub struct ModelList {
    pub data: Vec<Model>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub chat: Chat,
    #[serde(default)]
    pub pricing: Pricing,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Model {
    /// The harness converters only emit chat models (`type == "model"` and
    /// `kind == "chat"`).
    pub fn is_chat(&self) -> bool {
        self.r#type.as_deref().unwrap_or("model") == "model"
            && self.kind.as_deref().unwrap_or("chat") == "chat"
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Limits {
    #[serde(default)]
    pub max_context_window_tokens: Option<u64>,
    #[serde(default)]
    pub max_prompt_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Chat {
    #[serde(default)]
    pub modalities: Modalities,
    #[serde(default)]
    pub reasoning: Option<Reasoning>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Modalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Reasoning {
    #[serde(default)]
    pub effort: Option<Effort>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Effort {
    #[serde(default)]
    pub supported: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)] // round-tripped from /v1/models, kept for completeness
    pub default: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Pricing {
    #[serde(default)]
    pub entries: Vec<PricingEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PricingEntry {
    #[serde(default)]
    pub selector: Option<serde_json::Value>,
    #[serde(default)]
    pub rates: Rates,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Rates {
    #[serde(default)]
    pub input_tokens: Option<String>,
    #[serde(default)]
    pub output_tokens: Option<String>,
    #[serde(default)]
    pub input_cache_read_tokens: Option<String>,
    #[serde(default)]
    pub input_cache_write_tokens: Option<String>,
}

impl Rates {
    /// The default (selector-less) pricing entry, as the converters pick it.
    pub fn default_entry(pricing: &Pricing) -> Option<&Rates> {
        pricing
            .entries
            .iter()
            .find(|entry| entry.selector.is_none())
            .map(|entry| &entry.rates)
    }

    /// Decimal-string rates scale by 1e6 into per-million float costs.
    pub fn scaleb6(value: &str) -> Option<f64> {
        value.trim().parse::<f64>().ok().map(|v| v * 1e6)
    }
}
