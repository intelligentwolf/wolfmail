//! The background pass that runs the spoofing / fraud checks (crate::safety) over mail as it arrives and over the mail
//! already on this computer, and the "This sender is fine" commands.

use super::Engine;
use crate::error::{ApiError, Result};
use crate::safety;
use std::time::Duration;

/// Messages checked per pass. Each is parsed from its stored body on the read connection, so the writer (sync) is only
/// held for the short write of the verdicts.
const BATCH: i64 = 200;
/// How long to wait when nothing is left to check; new mail is picked up on the next pass.
const IDLE: Duration = Duration::from_secs(5);

impl Engine {
    pub(crate) fn spawn_safety_scanner(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                let n = match engine.safety_pass().await {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::warn!("safety checks: {}", e.message);
                        0
                    }
                };
                // A full batch means more is waiting (the first scan of a big mailbox): carry on after a breath.
                tokio::time::sleep(if n as i64 >= BATCH { Duration::from_millis(200) } else { IDLE }).await;
            }
        });
    }

    /// One batch: work out the verdicts, write them, tell the lists that changed. → how many were checked.
    async fn safety_pass(&self) -> Result<usize> {
        let verdicts = self.store().read(|c| safety::scan_batch(c, BATCH)).await?;
        let n = verdicts.len();
        if n == 0 {
            return Ok(0);
        }
        let changed = self.store().call(move |c| safety::store_verdicts(c, &verdicts)).await?;
        if !changed.is_empty() {
            self.emit("mail://changed", serde_json::json!({ "folderIds": changed }));
        }
        Ok(n)
    }

    /// "This sender is fine" (`trusted` true) or take that back: remembered for the account; the sender's mail is
    /// checked again at once.
    pub async fn message_trust_sender(&self, id: String, trusted: bool) -> Result<()> {
        let rid: i64 = id.parse().map_err(|_| ApiError::not_found("The message"))?;
        self.store().call(move |c| safety::set_trusted(c, rid, trusted).map(|_| ())).await?;
        // Most senders have far fewer messages than a batch; anything left is picked up by the scanner.
        self.safety_pass().await?;
        Ok(())
    }
}
