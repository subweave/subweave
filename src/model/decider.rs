//! The decider: a System One model answering typed questions with probabilities,
//! `POST {endpoint}/systemone`.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ModelRef;
use super::provider::{self, Provider};

/// Dollars per input token when the provider reports no cost.
const INPUT_PRICE: f64 = 0.042 / 1e6;
const RETRIES: u32 = 3;

/// Question and option order is kept as written: the measured wordings were sent in
/// that order.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// OpenRouter needs `criteria`, not `options`, or it answers 400.
    Choice {
        criteria: IndexMap<String, String>,
        instructions: String,
    },
    Noul {
        instructions: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: IndexMap<String, f64>,
        confidence: f64,
    },
    /// The probability of yes.
    Noul { noul: f64 },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Answers {
    pub answers: IndexMap<String, Answer>,
    pub usage: Usage,
    pub cost_usd: f64,
}

impl Answers {
    /// The probability of yes, or `None` when the question has no noul answer.
    pub fn noul(&self, key: &str) -> Option<f64> {
        match self.answers.get(key)? {
            Answer::Noul { noul } => Some(*noul),
            Answer::Choice { .. } => None,
        }
    }

    /// The choice, its option probabilities and the confidence.
    pub fn choice(&self, key: &str) -> Option<(&str, &IndexMap<String, f64>, f64)> {
        match self.answers.get(key)? {
            Answer::Choice {
                choice,
                probabilities,
                confidence,
            } => Some((choice, probabilities, *confidence)),
            Answer::Noul { .. } => None,
        }
    }
}

#[derive(Deserialize)]
struct Response {
    answers: IndexMap<String, Answer>,
    #[serde(default)]
    usage: Usage,
}

pub struct Decider {
    pub model: ModelRef,
    provider: Provider,
    key: Option<String>,
    client: reqwest::Client,
}

impl Decider {
    pub fn new(model: ModelRef) -> Result<Decider> {
        let provider = provider::get(&model.provider)?;
        let key = provider.key()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("starting the HTTP client")?;
        Ok(Decider {
            model,
            provider,
            key,
            client,
        })
    }

    /// The journal's record of one request.
    pub fn usage(&self, a: &Answers) -> super::Usage {
        super::Usage {
            model: self.model.to_string(),
            calls: 1,
            input: a.usage.input_tokens,
            cached: 0,
            output: a.usage.output_tokens,
            reasoning: 0,
        }
    }

    /// One request with every question. 429 and 5xx answers are retried 3 times; a
    /// failed call is an error, which callers read as no answer, never as yes.
    pub async fn ask(
        &self,
        state: Value,
        questions: IndexMap<String, Question>,
    ) -> Result<Answers> {
        let body = serde_json::json!({
            "model": self.model.id,
            "state": state,
            "questions": questions,
        });
        let url = self.provider.url("systemone");
        let mut attempt = 0;
        let text = loop {
            let mut req = self.client.post(&url).json(&body);
            if let Some(key) = &self.key {
                req = req.bearer_auth(key);
            }
            // A body read that fails or times out is an error, never an empty answer.
            let sent = match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    resp.text().await.map(|text| (status, text))
                }
                Err(e) => Err(e),
            };
            let retry = match sent {
                Ok((status, text)) => {
                    if status.is_success() {
                        break text;
                    }
                    let err = anyhow!(
                        "decider {}: HTTP {}: {}",
                        self.model,
                        status.as_u16(),
                        short(&text)
                    );
                    if !(status.as_u16() == 429 || status.is_server_error()) {
                        return Err(err);
                    }
                    err
                }
                Err(e) if e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() => {
                    anyhow!("decider {}: {e}", self.model)
                }
                Err(e) => return Err(anyhow!("decider {}: {e}", self.model)),
            };
            if attempt == RETRIES {
                return Err(retry);
            }
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            attempt += 1;
        };
        let resp: Response = serde_json::from_str(&text).with_context(|| {
            format!(
                "decider {}: unreadable answer: {}",
                self.model,
                short(&text)
            )
        })?;
        for key in questions.keys() {
            if !resp.answers.contains_key(key) {
                bail!("decider {}: no answer to {key}", self.model);
            }
        }
        let cost_usd = resp
            .usage
            .cost
            .unwrap_or(resp.usage.input_tokens as f64 * INPUT_PRICE);
        Ok(Answers {
            answers: resp.answers,
            usage: resp.usage,
            cost_usd,
        })
    }
}

fn short(text: &str) -> String {
    let t = text.trim();
    match t.char_indices().nth(300) {
        Some((at, _)) => format!("{}…", &t[..at]),
        None => t.to_string(),
    }
}
