use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
pub struct Message {
    pub op: isize,
    pub d: Option<Value>,
    pub s: Option<isize>,
    pub t: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct HeartbeatMessage {
    pub op: isize,
    pub d: HeartbeatData,
}

#[derive(Serialize, Deserialize)]
pub struct HeartbeatData {
    pub heartbeat_interval: isize,
}
