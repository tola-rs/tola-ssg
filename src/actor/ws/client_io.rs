use std::net::TcpStream;
use std::sync::Arc;

use parking_lot::Mutex;
use tungstenite::WebSocket;
use tungstenite::protocol::Message;

use crate::core::UrlPath;
use crate::reload::active::ACTIVE_PAGE;
use crate::reload::message::HotReloadMessage;

use super::{RegisteredClient, WsActor};

impl WsActor {
    /// Add a new client connection
    pub(super) fn add_client(&self, stream: TcpStream) {
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

                // Try to read initial page message immediately to avoid race condition
                // where hot reload message is sent before client's route is set
                let route = Self::try_read_initial_route(&mut ws);

                let mut clients = self.clients.lock();
                crate::debug!("ws"; "client connected (total: {}, route: {:?})", clients.len() + 1, route);
                clients.push(RegisteredClient { ws, route });
            }
            Err(e) => {
                crate::log!("ws"; "handshake failed: {}", e);
            }
        }
    }

    /// Background thread to read client messages (non-blocking poll)
    pub(super) fn client_reader_loop(clients: Arc<Mutex<Vec<RegisteredClient>>>) {
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

    /// Try to read initial page message from client
    ///
    /// Client sends `{type: "page", path: "/..."}` in onopen callback.
    /// We try to read it immediately to avoid race condition where
    /// hot reload message is sent before client's route is set.
    fn try_read_initial_route(ws: &mut WebSocket<TcpStream>) -> Option<UrlPath> {
        // Try multiple times with short delays to catch the initial message
        for _ in 0..5 {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    if let Some(route) = Self::parse_page_message(&text) {
                        ACTIVE_PAGE.add(route.clone());
                        return Some(route);
                    }
                }
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    // Message not yet arrived, wait a bit
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                _ => break,
            }
        }
        None
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
}
