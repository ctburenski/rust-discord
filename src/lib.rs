mod gateway;

#[macro_use]
extern crate log;

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{error::Error, fs, sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    select,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Mutex,
    },
};
use tokio_tungstenite::{tungstenite::client::IntoClientRequest, MaybeTlsStream, WebSocketStream};

// type alias for a long type name
type WebSocketSinkHandle =
    SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, tokio_tungstenite::tungstenite::Message>;

type WebSocketStreamHandle = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

pub struct Bot {
    config: Config,
    gateway_url: Option<String>,
    session_starts_remaining: Option<isize>,
    session_starts_total: Option<isize>,
    heartbeat_interval: Arc<Mutex<Option<isize>>>,
    websocket_sink: Arc<Mutex<Option<WebSocketSinkHandle>>>,
    websocket_stream: Arc<Mutex<Option<WebSocketStreamHandle>>>,
}

impl Bot {
    /// Creates a new instance of a Bot
    ///
    /// # Panics
    /// Will panic if it cannot acccess the config file or if that file is not valid JSON with a 'token' field
    pub fn new(config_file: &str) -> Bot {
        let config = fs::read_to_string(config_file)
            .expect(&format!("Failed to read config file: {}", config_file));

        trace!("Read config from {}", config_file);

        let config = serde_json::from_str::<Config>(&config)
            .expect("Config was not the valid format - should be valid JSON with a 'token' field.");

        Bot {
            config,
            gateway_url: None,
            session_starts_remaining: None,
            session_starts_total: None,
            heartbeat_interval: Arc::new(Mutex::new(None)),
            websocket_sink: Arc::new(Mutex::new(None)),
            websocket_stream: Arc::new(Mutex::new(None)),
        }
    }

    pub fn send_heartbeat(&self, mut rx: Receiver<()>) {
        let websocket = self.websocket_sink.clone();
        let heartbeat_interval = self.heartbeat_interval.clone();

        // this async tasks just lives on its own from now on
        // it only cares about the channels it has and the
        // copies of the heartbeat interval and the sink
        tokio::spawn(async move {
            // wait before beginning the heartbeat loop
            rx.recv().await;
            trace!("Started sending regular heartbeat...");
            loop {
                // get the time to wait
                let heartbeat_interval = heartbeat_interval.lock().await;
                // TODO his doesn't know which awaitable task returns
                select! {
                    // TODO this just causes the delay to short circuit but this isn't what we want
                    _ = {rx.recv() } => {
                    },
                    _ = {
                        // wait for the time between heartbeats
                        // TODO, is this the right idea?
                        tokio::time::sleep(Duration::from_millis(heartbeat_interval.unwrap().try_into().unwrap()))
                    } => {}
                }
                // create the heartbeat message
                let message = tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!(
                        {
                            "op": 1,
                            "d": heartbeat_interval.unwrap()
                        }
                    )
                    .to_string(),
                );
                // send the message - also, this method chaining is a bit much
                // TODO - maybe we should reduce the need for so many methods
                websocket
                    .lock()
                    .await
                    .as_mut()
                    .expect("Expected to get WebSocket, got nothing instead")
                    .send(message)
                    .await
                    .expect("Could not send initial heartbeat message");

                trace!("Heartbeat sent");
            }
        });
    }

    pub fn gateway_listener(&self, tx: Sender<()>) {
        let websocket_stream = self.websocket_stream.clone();
        let heartbeat_interval = self.heartbeat_interval.clone();
        let websocket_sink = self.websocket_sink.clone();
        tokio::spawn(async move {
            let mut websocket_stream = websocket_stream.lock().await;

            while let Some(m) = websocket_stream.as_mut().unwrap().next().await {
                if let Ok(msg) = &m {
                    trace!("Message received from Gateway: {}", msg.to_string());
                    let msg = serde_json::from_str::<gateway::Message>(&msg.to_string()).unwrap();

                    // maybe matching these methods to an enum instead of
                    // just using code comments would be better?
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
                            *heartbeat_interval.lock().await = Some(
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
                                heartbeat_interval.lock().await.unwrap()
                            );
                            let message = tokio_tungstenite::tungstenite::Message::Text(
                                serde_json::json!(
                                    {
                                        "op": 1,
                                        "d": heartbeat_interval.lock().await.unwrap()
                                    }
                                )
                                .to_string(),
                            );
                            websocket_sink
                                .lock()
                                .await
                                .as_mut()
                                .unwrap()
                                .send(message)
                                .await
                                .expect("Could not send initial heartbeat message");
                            tx.send(()).await.unwrap();
                            trace!("Hello event was ok, initial heartbeat sent");
                            continue;
                        }
                        // TODO will have to handle not receiving this event when expecting to
                        // heartbeat acknowledged
                        11 => continue,
                        _ => continue,
                    }
                }
            }
        });
    }

    pub async fn run(self) {
        self.initialize().await.expect("Failed to initialize")
    }

    /// initialize will get the bot connected to the gateway and sending regular
    /// heartbeats
    async fn initialize(mut self) -> Result<(), Box<dyn Error>> {
        // We use this channel to alert the heartbeat loop
        // to begin sending the heartbeat at regular intervals
        let (tx, rx) = channel::<()>(1);

        self.send_heartbeat(rx);

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

        trace!("{}/?v=10&encoding=json", self.gateway_url.as_ref().unwrap());

        // connect to gateway
        let gateway_connection = tokio_tungstenite::connect_async(
            format!(
                "{}/?v=10&encoding=json",
                self.gateway_url
                    .as_ref()
                    .expect("No URL stored for gateway")
            )
            .into_client_request()
            .unwrap(),
        )
        .await
        .unwrap();

        trace!("Connected to discord gateway.");

        let (ws, _) = gateway_connection;
        let (sink, stream) = ws.split();
        *self.websocket_sink.lock().await = Some(sink);
        *self.websocket_stream.lock().await = Some(stream);

        self.gateway_listener(tx);

        Ok(())
    }
}

// TODO we should make sure this token is never logged
#[derive(Serialize, Deserialize)]
struct Config {
    token: String,
}
