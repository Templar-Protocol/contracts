//! Telegram bot client.
//!
//! Sends formatted alert reports to Telegram channels or specific threads within groups.
//! Includes automatic retry logic for rate limiting (HTTP 429).

use crate::error::{MonitorError, Result};
use reqwest::Client;
use serde_json::json;

pub struct TelegramClient {
    bot_token: String,
    client: Client,
}

impl TelegramClient {
    /// Creates a new Telegram client with the given bot token.
    pub fn new(bot_token: String) -> Self {
        Self {
            bot_token,
            client: Client::new(),
        }
    }

    /// Sends a message to a Telegram chat.
    ///
    /// # Arguments
    /// * `chat_id` - The target chat ID (channel or group)
    /// * `text` - The message text (HTML formatting supported)
    /// * `thread_id` - Optional thread/topic ID for posting to specific threads in supergroups
    ///
    /// # Errors
    /// Returns an error if the request fails or rate limiting cannot be resolved.
    pub async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        thread_id: Option<i64>,
    ) -> Result<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);

        tracing::debug!(chat_id = %chat_id, text_len = text.len(), "Sending Telegram message");

        let mut payload = json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });

        if let Some(tid) = thread_id {
            payload["message_thread_id"] = json!(tid);
        }

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| MonitorError::Telegram(format!("Failed to send request: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            // Handle rate limiting
            if status == 429 {
                tracing::warn!("Telegram rate limit hit, waiting 60s before retry");
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

                // Retry once
                let retry_response = self
                    .client
                    .post(&url)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| MonitorError::Telegram(format!("Retry failed: {e}")))?;

                if !retry_response.status().is_success() {
                    return Err(MonitorError::Telegram(format!(
                        "Retry failed with status {}: {}",
                        retry_response.status(),
                        retry_response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string())
                    )));
                }

                tracing::info!("Telegram message sent successfully (after retry)");
                return Ok(());
            }

            return Err(MonitorError::Telegram(format!(
                "HTTP {status}: {error_text}"
            )));
        }

        tracing::info!("Telegram message sent successfully");
        Ok(())
    }
}
