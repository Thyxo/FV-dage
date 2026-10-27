//! Klient til cell-managers REST-API. Appen skriver KUN `daysLeft`, og kun
//! værdier der lige er verificeret med /ce info. Alle andre felter ejes af
//! cell-manager.

use anyhow::{anyhow, bail, Result};
use reqwest::{Client, RequestBuilder, Url};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCell {
    #[serde(rename = "_id")]
    pub id: String,
    pub cell_name: String,
    #[serde(default)]
    pub account_name: String,
    #[serde(default)]
    pub rank: String,
    #[serde(default)]
    pub block: String,
    #[serde(default)]
    pub days_left: i64,
}

pub struct Manager {
    base: Url,
    api_key: Option<String>,
    http: Client,
}

impl Manager {
    pub fn new(url: &str, api_key: Option<String>) -> Result<Manager> {
        let url = url.trim();
        if url.is_empty() {
            bail!("Ingen cell-manager-adresse er sat");
        }
        let base = Url::parse(url).map_err(|_| anyhow!("Ugyldig adresse: {url}"))?;
        let http = Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("CellKeeper")
            .build()?;
        Ok(Manager { base, api_key: api_key.filter(|k| !k.trim().is_empty()), http })
    }

    fn url(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow!("Ugyldig adresse"))?
            .pop_if_empty()
            .extend(segments); // URL-encoder fx mellemrum i cellenavne
        Ok(url)
    }

    fn auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.api_key {
            Some(k) => req.header("x-api-key", k.trim()),
            None => req,
        }
    }

    pub async fn cells(&self) -> Result<Vec<RemoteCell>> {
        let res = self
            .auth(self.http.get(self.url(&["api", "cells"])?))
            .send()
            .await
            .map_err(|e| anyhow!("Kunne ikke nå cell-manager: {e}"))?;
        if !res.status().is_success() {
            bail!("cell-manager svarede {}", res.status());
        }
        Ok(res.json().await?)
    }

    pub async fn set_days(&self, cell_name: &str, days: u8) -> Result<()> {
        let days = days.min(9); // cell-manager tillader kun 0-9
        let res = self
            .auth(self.http.patch(self.url(&["api", "cells", cell_name, "days"])?))
            .json(&serde_json::json!({ "daysLeft": days, "source": "cellkeeper" }))
            .send()
            .await
            .map_err(|e| anyhow!("Kunne ikke nå cell-manager: {e}"))?;
        match res.status().as_u16() {
            200..=299 => Ok(()),
            401 | 403 => bail!("cell-manager afviste API-nøglen"),
            404 => bail!("Cellen {cell_name} findes ikke i cell-manager"),
            s => {
                let body = res.text().await.unwrap_or_default();
                bail!("cell-manager svarede {s}: {}", body.chars().take(200).collect::<String>())
            }
        }
    }

    /// Tjekker forbindelse og at API-nøglen virker (hvis serveren kræver en).
    pub async fn test(&self) -> Result<String> {
        let cells = self.cells().await?;
        let res = self
            .auth(self.http.patch(self.url(&["api", "cells", "__cellkeeper_test__", "days"])?))
            .json(&serde_json::json!({ "daysLeft": 0 }))
            .send()
            .await?;
        // Testcellen findes ikke: 404 betyder at skriveadgang er i orden.
        let key_note = match res.status().as_u16() {
            401 | 403 => "API-nøglen mangler eller er forkert",
            404 => "Skriveadgang OK",
            s => return Ok(format!("Forbundet: {} celler. Uventet svar {s} ved skrivetest.", cells.len())),
        };
        Ok(format!("Forbundet: {} celler. {key_note}.", cells.len()))
    }
}
