//! Owns the desktop's live viewer connection. One per app.
use air_rs::inbox::client::ClientHandle;
use std::sync::Mutex;

/// Holds the live viewer client handle (None until inbox_start). Send acks round-trip to the UI as
/// Tauri events, so no correlation map is kept here.
#[derive(Default)]
pub struct InboxManager {
    /// The live viewer client (None until inbox_start). Stopped on app exit / inbox_stop.
    pub viewer: Mutex<Option<ClientHandle>>,
}

impl InboxManager {
    /// Construct an empty manager.
    pub fn new() -> Self {
        Self::default()
    }
}
