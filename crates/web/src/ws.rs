//! WebSocket client with exponential-backoff reconnect.
//!
//! Frames are JSON deltas (`metric_delta`, `agent_status`, `personal_metrics`,
//! `snapshot`). Each frame is applied to the existing signals, so Leptos only
//! re-renders the affected cards — never the whole page.

use futures::{SinkExt, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::Message;
use gloo_timers::future::TimeoutFuture;
use leptos::*;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Live,
    Retrying,
}

impl ConnState {
    pub fn label(self) -> &'static str {
        match self {
            ConnState::Connecting => "Connecting…",
            ConnState::Live => "Live",
            ConnState::Retrying => "Reconnecting…",
        }
    }
}

fn url(token: &str) -> String {
    let origin = crate::api::origin();
    let ws_origin = origin.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    format!("{ws_origin}/ws/dashboard?token={token}")
}

/// Connect and keep reconnecting until the returned handle is dropped.
pub fn connect(token: String, on_frame: Callback<Value>, state: RwSignal<ConnState>) {
    spawn_local(async move {
        let mut attempt: u32 = 0;
        loop {
            state.set(if attempt == 0 {
                ConnState::Connecting
            } else {
                ConnState::Retrying
            });
            match WebSocket::open(&url(&token)) {
                Ok(mut socket) => {
                    let mut got_message = false;
                    while let Some(message) = socket.next().await {
                        match message {
                            Ok(Message::Text(text)) => {
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    if !got_message {
                                        got_message = true;
                                        attempt = 0;
                                        state.set(ConnState::Live);
                                    }
                                    on_frame.call(value);
                                }
                            }
                            Ok(Message::Bytes(bytes)) => {
                                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                                    if !got_message {
                                        got_message = true;
                                        attempt = 0;
                                        state.set(ConnState::Live);
                                    }
                                    on_frame.call(value);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    // The peer closed; fall through to the backoff below.
                }
                Err(_) => {}
            }

            state.set(ConnState::Retrying);
            attempt = attempt.saturating_add(1);
            // 1s, 2s, 4s … capped at 30s, with a small jitter.
            let backoff_ms = u32::min(30_000, 500 * 2u32.pow(attempt.min(6)));
            let jitter = (js_sys::Math::random() * 400.0) as u32;
            TimeoutFuture::new(backoff_ms + jitter).await;
        }
    });
}
