//! Pairing code system — one-time codes for authorizing Telegram chat IDs.
//!
//! Flow:
//!   1. `arli pair generate` → writes a 6-char code + expiry to `~/.arli/pairing_code`
//!   2. User sends `arli pair <code>` to the Telegram bot
//!   3. Gateway verifies the code, adds chat_id to `~/.arli/allowed_users.json`
//!   4. Future messages from that chat_id are processed normally

use std::path::Path;

pub const PAIRING_CODE_VALIDITY_SECS: u64 = 600; // 10 minutes

/// Generate a random 6-character alphanumeric code (no confusable chars).
pub fn generate_code() -> String {
    use rand::Rng;
    let chars: Vec<char> = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".chars().collect();
    let mut rng = rand::thread_rng();
    (0..6).map(|_| chars[rng.gen_range(0..chars.len())]).collect()
}

/// Verify a pairing code against the stored one-time code.
/// Returns true if the code matches and is not expired.
/// Consumes the code file on successful match (one-time use).
pub fn verify(code: &str, data_dir: &Path) -> bool {
    let pairing_file = data_dir.join("pairing_code");
    let contents = match std::fs::read_to_string(&pairing_file) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let mut lines = contents.lines();
    let stored_code = match lines.next() {
        Some(c) => c.trim().to_string(),
        None => return false,
    };
    let expires_at: u64 = match lines.next().and_then(|l| l.trim().parse().ok()) {
        Some(e) => e,
        None => return false,
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if now > expires_at {
        let _ = std::fs::remove_file(&pairing_file);
        return false;
    }

    let matches = code.len() == stored_code.len()
        && code
            .bytes()
            .zip(stored_code.bytes())
            .all(|(a, b)| a.eq_ignore_ascii_case(&b));

    if matches {
        let _ = std::fs::remove_file(&pairing_file);
    }

    matches
}

/// Persistent list of allowed Telegram chat IDs.
///
/// Stored as JSON array in `allowed_users.json`.
pub struct AllowedUsers {
    chat_ids: Vec<i64>,
}

impl AllowedUsers {
    /// Load the list from disk (or return empty if not found).
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("allowed_users.json");
        let chat_ids = match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str::<Vec<i64>>(&contents).unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        Self { chat_ids }
    }

    /// Check whether a chat_id is authorized.
    pub fn is_allowed(&self, chat_id: i64) -> bool {
        self.chat_ids.contains(&chat_id)
    }

    /// Authorize a new chat_id and persist.
    pub fn add(&mut self, chat_id: i64, data_dir: &Path) -> std::io::Result<()> {
        if !self.chat_ids.contains(&chat_id) {
            self.chat_ids.push(chat_id);
            let path = data_dir.join("allowed_users.json");
            let json = serde_json::to_string_pretty(&self.chat_ids)?;
            std::fs::write(&path, json)?;
        }
        Ok(())
    }

    /// Number of authorized users.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.chat_ids.len()
    }
}