//! WebSocket Actor - Bidirectional Communication
//!
//! This actor is responsible for:
//! - Managing WebSocket client connections
//! - Broadcasting messages to all connected clients
//! - Targeted push to clients viewing specific routes
//! - Receiving client messages (e.g., current page URL)
//!
//! # Architecture
//!
//! ```text
//! VdomActor --[Patch/Reload]--> WsActor --[targeted/broadcast]--> Clients
//!                                  ^                                  |
//!                                  +----------[page URL]--------------+
//! ```

mod client_io;
mod delivery;

use std::net::TcpStream;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;
use tungstenite::WebSocket;
use tungstenite::protocol::Message;

use super::messages::WsMsg;
use crate::cache::PersistedError;
use crate::core::UrlPath;
use crate::reload::active::ACTIVE_PAGE;
use crate::reload::message::HotReloadMessage;

/// A registered WebSocket client with its current route
struct RegisteredClient {
    ws: WebSocket<TcpStream>,
    /// Current route this client is viewing (for targeted push)
    route: Option<UrlPath>,
}

/// WebSocket Actor - manages client connections and broadcasts
pub struct WsActor {
    /// Channel to receive messages
    rx: mpsc::Receiver<WsMsg>,
    /// Connected clients (shared for broadcast + read threads)
    clients: Arc<Mutex<Vec<RegisteredClient>>>,
    /// Pending error to send to new clients (snapshot recovery)
    pending_error: Arc<Mutex<Option<PersistedError>>>,
}

impl WsActor {
    /// Create a new WsActor
    pub fn new(rx: mpsc::Receiver<WsMsg>) -> Self {
        Self {
            rx,
            clients: Arc::new(Mutex::new(Vec::new())),
            pending_error: Arc::new(Mutex::new(None)),
        }
    }

    /// Set initial pending error (for snapshot recovery)
    pub fn with_pending_error(self, path: String, error: String) -> Self {
        *self.pending_error.lock() = Some(PersistedError::new(path, String::new(), error));
        self
    }

    /// Run the actor event loop
    pub async fn run(mut self) {
        // Spawn a background task to poll client messages
        let clients_for_reader = Arc::clone(&self.clients);
        std::thread::spawn(move || {
            Self::client_reader_loop(clients_for_reader);
        });

        while let Some(msg) = self.rx.recv().await {
            match msg {
                WsMsg::Patch {
                    url_path,
                    patches,
                    url_change,
                } => {
                    // Build HotReloadMessage with optional url_change
                    let hr_msg = if let Some(change) = url_change {
                        crate::debug!("ws"; "sending patch with url_change: {} -> {}", change.old, change.new);
                        HotReloadMessage::patch_with_url_change(
                            url_path.as_str(),
                            Self::convert_patches(&patches),
                            crate::reload::message::UrlChange {
                                old: change.old,
                                new: change.new,
                            },
                        )
                    } else {
                        HotReloadMessage::from_patches(url_path.as_str(), &patches)
                    };
                    // Targeted push: only send to clients viewing this route
                    self.send_to_route(&url_path, Message::Text(hr_msg.to_json().into()));
                }

                WsMsg::Reload {
                    reason,
                    url_path,
                    url_change,
                } => {
                    crate::debug!("ws"; "sending reload: {}", reason);
                    let hr_msg = if let Some(change) = url_change {
                        crate::debug!("ws"; "reload with url_change: {} -> {}", change.old, change.new);
                        HotReloadMessage::reload_with_url_change(
                            &reason,
                            crate::reload::message::UrlChange {
                                old: change.old,
                                new: change.new,
                            },
                        )
                    } else {
                        HotReloadMessage::reload_with_reason(&reason)
                    };
                    // Targeted or broadcast based on url_path
                    if let Some(ref route) = url_path {
                        self.send_to_route(route, Message::Text(hr_msg.to_json().into()));
                    } else {
                        self.broadcast(Message::Text(hr_msg.to_json().into()));
                    }
                }

                WsMsg::Error { path, error } => {
                    // Cache error for new clients (snapshot recovery)
                    *self.pending_error.lock() = Some(PersistedError::new(
                        path.clone(),
                        String::new(),
                        error.clone(),
                    ));
                    let hr_msg = HotReloadMessage::error(&path, &error);
                    self.broadcast(Message::Text(hr_msg.to_json().into()));
                }

                WsMsg::ClearError => {
                    // Clear pending error
                    *self.pending_error.lock() = None;
                    let hr_msg = HotReloadMessage::clear_error();
                    self.broadcast(Message::Text(hr_msg.to_json().into()));
                }

                WsMsg::AddClient(stream) => {
                    self.add_client(stream);
                }

                WsMsg::ClientConnected => {
                    crate::debug!("ws"; "client notification received");
                }

                WsMsg::Shutdown => {
                    crate::debug!("ws"; "shutting down");
                    let mut clients = self.clients.lock();
                    for mut client in clients.drain(..) {
                        let _ = client.ws.close(None);
                    }
                    ACTIVE_PAGE.clear();
                    break;
                }
            }
        }
    }
}
