use tungstenite::protocol::Message;

use crate::core::UrlPath;
use crate::reload::active::ACTIVE_PAGE;

use super::WsActor;

impl WsActor {
    /// Convert VDOM Patches to PatchOps for HotReloadMessage
    pub(super) fn convert_patches(
        patches: &[tola_vdom::algo::Patch],
    ) -> Vec<crate::reload::patch::PatchOp> {
        crate::reload::patch::from_vdom_patches(patches)
    }

    /// Broadcast a message to all connected clients
    pub(super) fn broadcast(&self, msg: Message) {
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
    pub(super) fn send_to_route(&self, target_route: &UrlPath, msg: Message) {
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
