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

    /// Convert VDOM Patches to PatchOps for HotReloadMessage
    fn convert_patches(patches: &[tola_vdom::algo::Patch]) -> Vec<crate::reload::patch::PatchOp> {
        crate::reload::patch::from_vdom_patches(patches)
    }

    /// Add a new client connection
    fn add_client(&self, stream: TcpStream) {
        // Keep blocking mode during handshake, switch to non-blocking after
        match tungstenite::accept(stream) {
            Ok(mut ws) => {
                // Now set non-blocking for polling reads
                let _ = ws.get_ref().set_nonblocking(true);

                // Send connected message
                let connected_msg = HotReloadMessage::connected();
                if let Err(e) = ws.send(Message::Text(connected_msg.to_json().into())) {
                    crate::log!("ws"; "failed to send connected message: {}", e);
                    return;
                }

                // Send pending error if any (snapshot recovery)
                if let Some(ref err) = *self.pending_error.lock() {
                    let (path, error) = err.details();
                    let hr_msg = HotReloadMessage::error(path, error);
                    if let Err(e) = ws.send(Message::Text(hr_msg.to_json().into())) {
                        crate::log!("ws"; "failed to send pending error: {}", e);
                    } else {
                        crate::debug!("ws"; "sent pending error to new client");
                    }
                }

                let mut clients = self.clients.lock();
                crate::debug!("ws"; "client connected (total: {})", clients.len() + 1);
                clients.push(RegisteredClient { ws, route: None });
            }
            Err(e) => {
                crate::log!("ws"; "handshake failed: {}", e);
            }
        }
    }

    /// Background thread to read client messages (non-blocking poll)
    fn client_reader_loop(clients: Arc<Mutex<Vec<RegisteredClient>>>) {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(100));

            let mut clients_guard = clients.lock();
            let mut disconnected = Vec::new();

            for (i, client) in clients_guard.iter_mut().enumerate() {
                // Non-blocking read
                match client.ws.read() {
                    Ok(Message::Text(text)) => {
                        // Parse and update client's route
                        if let Some(new_route) = Self::parse_page_message(&text) {
                            // Remove old route from ACTIVE_PAGE if different
                            if let Some(ref old_route) = client.route
                                && old_route != &new_route
                            {
                                ACTIVE_PAGE.remove(old_route);
                            }
                            // Add new route to ACTIVE_PAGE
                            ACTIVE_PAGE.add(new_route.clone());
                            client.route = Some(new_route);
                        }
                    }
                    Ok(Message::Close(_)) => {
                        disconnected.push(i);
                    }
                    Err(tungstenite::Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        // No data available, continue
                    }
                    Err(_) => {
                        disconnected.push(i);
                    }
                    _ => {}
                }
            }

            // Remove disconnected clients and their routes from ACTIVE_PAGE
            for i in disconnected.into_iter().rev() {
                if let Some(ref route) = clients_guard[i].route {
                    ACTIVE_PAGE.remove(route);
                }
                clients_guard.remove(i);
            }

            // Clear active page if no clients
            if clients_guard.is_empty() {
                ACTIVE_PAGE.clear();
            }
        }
    }

    /// Parse page message and return the route if valid
    fn parse_page_message(text: &str) -> Option<UrlPath> {
        use percent_encoding::percent_decode_str;

        let json = serde_json::from_str::<serde_json::Value>(text).ok()?;
        if json.get("type").and_then(|t| t.as_str()) != Some("page") {
            return None;
        }
        let path = json.get("path").and_then(|p| p.as_str())?;
        let decoded = percent_decode_str(path)
            .decode_utf8()
            .unwrap_or_else(|_| path.into());
        crate::debug!("ws"; "client route: {}", decoded);
        Some(UrlPath::from_page(&decoded))
    }

    /// Broadcast a message to all connected clients
    fn broadcast(&self, msg: Message) {
        let mut clients = self.clients.lock();
        let count = clients.len();

        if count == 0 {
            crate::debug!("ws"; "no clients connected");
            return;
        }

        clients.retain_mut(|client| match client.ws.send(msg.clone()) {
            Ok(_) => true,
            Err(e) => {
                crate::debug!("ws"; "client disconnected: {}", e);
                if let Some(ref route) = client.route {
                    ACTIVE_PAGE.remove(route);
                }
                false
            }
        });
        crate::debug!("ws"; "broadcast to {} clients", count);
    }

    /// Send a message to clients viewing a specific route
    fn send_to_route(&self, target_route: &UrlPath, msg: Message) {
        let mut clients = self.clients.lock();
        let mut sent = 0;

        clients.retain_mut(|client| {
            // Check if client is viewing this route
            let matches = client
                .route
                .as_ref()
                .map(|r| r.matches_ignoring_trailing_slash(target_route.as_str()))
                .unwrap_or(false);

            if matches {
                match client.ws.send(msg.clone()) {
                    Ok(_) => {
                        sent += 1;
                        true
                    }
                    Err(e) => {
                        crate::debug!("ws"; "client disconnected: {}", e);
                        if let Some(ref route) = client.route {
                            ACTIVE_PAGE.remove(route);
                        }
                        false
                    }
                }
            } else {
                true // Keep client, just don't send
            }
        });

        if sent > 0 {
            crate::debug!("ws"; "sent to {} clients viewing {}", sent, target_route);
        }
    }
}
