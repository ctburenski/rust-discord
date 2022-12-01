#[macro_use]
extern crate log;

use std::{error::Error, fs};

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub struct Bot {
    config: Config,
    gateway_url: Option<String>,
    session_starts_remaining: Option<isize>,
    session_starts_total: Option<isize>,
}

impl Bot {
    pub fn new(config_file: &str) -> Result<Bot, Box<dyn Error>> {
        let config = fs::read_to_string(config_file)?;
        trace!("Read config from {}", config_file);
        let config = serde_json::from_str::<Config>(&config)?;
        return Ok(Bot {
            config,
            gateway_url: None,
            session_starts_remaining: None,
            session_starts_total: None,
        });
    }

    pub async fn run(self) {
        self.initialize().await.expect("Failed to initialize")
    }

    async fn initialize(mut self) -> Result<(), Box<dyn Error>> {
        // TODO mapp all the errors in this instead of using except
        // eventually should return enums
        // sending a request to get the URL for the bot gateway
        let response = Client::new()
            .get("https://discord.com/api/v9/gateway/bot")
            .header("Authorization", format!("Bot {}", self.config.token))
            .send()
            .await
            .map_err(|_| "Couldn't authenticate to discord.com/api")?;
        debug!("Response status code is: {}", response.status());

        // ensuring it is valid JSON
        let response = response
            .json::<serde_json::Value>()
            .await
            .expect("Response from discord.com/api was not valid JSON");
        trace!("Actual response: {:#?}", response);

        // collecting the URL
        let gateway_url: String = response
            .get("url")
            .expect("URL was not included")
            .as_str()
            .unwrap()
            .into();
        self.gateway_url = Some(gateway_url.clone());
        trace!("Updated gateway url to: {}", gateway_url);

        // collecting session start limit info
        let session_starts = response
            .get("session_start_limit")
            .expect("session start object not included in JSON response");

        self.session_starts_remaining = Some(
            session_starts
                .get("remaining")
                .expect("remaining field not found in session start object")
                .to_string()
                .parse::<isize>()
                .expect("remaining tries was not a valid number"),
        );

        self.session_starts_total = Some(
            session_starts
                .get("total")
                .expect("total field not found in session start object")
                .to_string()
                .parse::<isize>()
                .expect("total tries was not a valid number"),
        );
        trace!(
            "Remaining starts out of total: {} / {}",
            self.session_starts_remaining.unwrap(),
            self.session_starts_remaining.unwrap()
        );

        // avoid connecting to gateway if we reached the limit
        if self.session_starts_remaining.is_some() && self.session_starts_remaining <= Some(0) {
            return Err("Session start limit reached!".into());
        }

        trace!(
            "{}/?v=10&encoding=json",
            self.gateway_url.as_ref().clone().unwrap()
        );

        // connect to gateway
        let gateway_connection = tokio_tungstenite::connect_async(
            format!(
                "{}/?v=10&encoding=json",
                self.gateway_url
                    .as_ref()
                    .clone()
                    .expect("No URL stored for gateway")
            )
            .into_client_request()
            .unwrap(),
        )
        .await
        .unwrap();

        trace!("Connected to discord gateway.");

        let (ws, _) = gateway_connection;
        let (sink, mut stream) = ws.split();

        return Ok(());
    }
}

#[derive(Serialize, Deserialize)]
struct Config {
    token: String,
}
