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
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tokio::sync::mpsc;
use tungstenite::WebSocket;
use tungstenite::protocol::Message;

use super::messages::WsMsg;
use crate::cache::{PersistedDiagnostics, PersistedError};
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
    /// Current error set to send to new clients (snapshot recovery)
    pending_errors: Arc<Mutex<PersistedDiagnostics>>,
    /// Stop signal for the reader thread.
    stop_reader: Arc<AtomicBool>,
}

impl WsActor {
    /// Create a new WsActor
    pub fn new(rx: mpsc::Receiver<WsMsg>) -> Self {
        Self {
            rx,
            clients: Arc::new(Mutex::new(Vec::new())),
            pending_errors: Arc::new(Mutex::new(PersistedDiagnostics::new())),
            stop_reader: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set one initial pending error (for snapshot recovery)
    pub fn with_pending_error(self, path: String, error: String) -> Self {
        self.with_pending_errors(vec![PersistedError::new(path, String::new(), error)])
    }

    /// Set all initial pending errors (for snapshot recovery)
    pub fn with_pending_errors(self, errors: Vec<PersistedError>) -> Self {
        {
            let mut pending = self.pending_errors.lock();
            for error in errors {
                pending.push_error(error);
            }
        }
        self
    }

    /// Run the actor event loop
    pub async fn run(mut self) {
        // Spawn a background task to poll client messages
        let clients_for_reader = Arc::clone(&self.clients);
        let stop_reader = Arc::clone(&self.stop_reader);
        let reader_handle = std::thread::spawn(move || {
            Self::client_reader_loop(clients_for_reader, stop_reader);
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
                    self.pending_errors.lock().push_error(PersistedError::new(
                        path.clone(),
                        String::new(),
                        error.clone(),
                    ));
                    let hr_msg = HotReloadMessage::error(&path, &error);
                    self.broadcast(Message::Text(hr_msg.to_json().into()));
                }

                WsMsg::ClearError { path } => {
                    self.pending_errors.lock().clear_errors_for(&path);
                    let hr_msg = HotReloadMessage::clear_error(&path);
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
                    break;
                }
            }
        }

        self.shutdown_clients();
        self.stop_reader.store(true, Ordering::SeqCst);
        let _ = reader_handle.join();
    }

    fn shutdown_clients(&self) {
        let mut clients = self.clients.lock();
        for mut client in clients.drain(..) {
            let _ = client.ws.close(None);
        }
        ACTIVE_PAGE.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::WsActor;
    use crate::core::UrlPath;
    use crate::reload::active::ACTIVE_PAGE;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn run_clears_active_pages_when_channel_closes() {
        ACTIVE_PAGE.add(UrlPath::from_page("/stale/"));

        let (tx, rx) = mpsc::channel(1);
        drop(tx);

        let actor = WsActor::new(rx);
        actor.run().await;

        assert!(ACTIVE_PAGE.get_all().is_empty());
    }
}
