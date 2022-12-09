#[macro_use]
extern crate log;

use std::{error::Error, fs, sync::Arc, time::Duration};

use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{self, Value};
use tokio::{
    net::TcpStream,
    select,
    sync::{
        mpsc::{channel, Sender},
        Mutex,
    },
};
use tokio_tungstenite::{tungstenite::client::IntoClientRequest, MaybeTlsStream, WebSocketStream};

pub struct Bot {
    config: Config,
    gateway_url: Option<String>,
    session_starts_remaining: Option<isize>,
    session_starts_total: Option<isize>,
    heartbeat_interval: Arc<Mutex<Option<isize>>>,
    websocket: Arc<
        Mutex<
            Option<
                SplitSink<
                    WebSocketStream<MaybeTlsStream<TcpStream>>,
                    tokio_tungstenite::tungstenite::Message,
                >,
            >,
        >,
    >,
}

impl Bot {
    /// Creates a new instance of a Bot
    pub fn new(config_file: &str) -> Result<Bot, Box<dyn Error>> {
        let config = fs::read_to_string(config_file)?;
        trace!("Read config from {}", config_file);
        let config = serde_json::from_str::<Config>(&config)?;
        return Ok(Bot {
            config,
            gateway_url: None,
            session_starts_remaining: None,
            session_starts_total: None,
            heartbeat_interval: Arc::new(Mutex::new(None)),
            websocket: Arc::new(Mutex::new(None)),
        });
    }

    pub async fn run(self) {
        let (tx, mut rx) = channel::<()>(100);
        let websocket = self.websocket.clone();
        let heartbeat_interval = self.heartbeat_interval.clone();
        tokio::spawn(async move {
            rx.recv().await;
            trace!("Started sending regular heartbeat...");
            loop {
                let heartbeat_interval = heartbeat_interval.lock().await;
                select! {
                    _ = {rx.recv() } => {
                    },
                    _ = {
                        trace!("Heartbeat should be positive: {}", heartbeat_interval.unwrap());
                        tokio::time::sleep(Duration::from_millis(heartbeat_interval.unwrap().try_into().unwrap()))
                    } => {}
                }
                let message = tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!(
                        {
                            "op": 1,
                            "d": heartbeat_interval.unwrap()
                        }
                    )
                    .to_string(),
                );
                websocket
                    .lock()
                    .await
                    .as_mut()
                    .unwrap()
                    .send(message)
                    .await
                    .expect("Could not send initial heartbeat message");
                trace!("Heartbeat sent");
            }
        });
        self.initialize(tx).await.expect("Failed to initialize")
    }

    async fn initialize(mut self, tx: Sender<()>) -> Result<(), Box<dyn Error>> {
        // TODO map all the errors in this instead of using except
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
        *self.websocket.lock().await = Some(sink);

        while let Some(m) = stream.next().await {
            if let Ok(msg) = &m {
                trace!("Message received from Gateway: {}", msg.to_string());
                let msg = serde_json::from_str::<Message>(&msg.to_string())?;

                match msg.op {
                    // dispatch code
                    0 => continue,
                    // heartbeat
                    1 => continue,
                    // identify
                    2 => continue,
                    // presence update
                    3 => continue,
                    // voice state update
                    4 => continue,
                    // resume
                    6 => continue,
                    // reconnect
                    7 => continue,
                    // guild member request
                    8 => continue,
                    // invalid session
                    9 => continue,
                    // hello
                    10 => {
                        trace!("Received hello event!");
                        *self.heartbeat_interval.lock().await = Some(
                            msg.d
                                .unwrap()
                                .get("heartbeat_interval")
                                .expect("Could not get heartbeat interval from hello message")
                                .to_string()
                                .parse::<isize>()
                                .unwrap(),
                        );
                        trace!(
                            "Heartbeat interval: {}",
                            self.heartbeat_interval.lock().await.unwrap()
                        );
                        let message = tokio_tungstenite::tungstenite::Message::Text(
                            serde_json::json!(
                                {
                                    "op": 1,
                                    "d": self.heartbeat_interval.lock().await.unwrap()
                                }
                            )
                            .to_string(),
                        );
                        self.websocket
                            .lock()
                            .await
                            .as_mut()
                            .unwrap()
                            .send(message)
                            .await
                            .expect("Could not send initial heartbeat message");
                        tx.send(()).await?;
                        trace!("Hello event was ok, initial heartbeat sent");
                        continue;
                    }
                    // heartbeat acknowledged
                    // TODO will have to handle not receiving this event when expecting to
                    11 => continue,
                    _ => continue,
                }
            }
        }

        return Ok(());
    }
}

#[derive(Serialize, Deserialize)]
struct Config {
    token: String,
}

#[derive(Serialize, Deserialize)]
struct Message {
    op: isize,
    d: Option<Value>,
    s: Option<isize>,
    t: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct HeartbeatMessage {
    op: isize,
    d: HeartbeatData,
}

#[derive(Serialize, Deserialize)]
struct HeartbeatData {
    heartbeat_interval: isize,
}
