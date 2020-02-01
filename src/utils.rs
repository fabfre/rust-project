use serde::{Deserialize, Serialize};
use std::time;

pub const THREAD_SLEEP_DURATION: time::Duration = time::Duration::from_millis(5000);

pub const HEARTBEAT_SLEEP_DURATION: time::Duration = time::Duration::from_secs(10);

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
pub enum Instructions {
    PLAY,
    GET,
    ORDER,
    REMOVE,
}

pub trait AppListener: Send {
    fn notify(&self);
}