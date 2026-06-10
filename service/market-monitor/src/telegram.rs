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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telegram_client_new() {
        let token = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11";
        let client = TelegramClient::new(token.to_string());
        assert_eq!(client.bot_token, token);
    }

    #[test]
    fn test_telegram_url_format() {
        let token = "test_token";
        let expected_url = "https://api.telegram.org/bottest_token/sendMessage";
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        assert_eq!(url, expected_url);
    }

    #[test]
    fn test_payload_without_thread() {
        let payload = json!({
            "chat_id": "-1001234567890",
            "text": "Test message",
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });
        assert_eq!(payload["chat_id"], "-1001234567890");
        assert_eq!(payload["text"], "Test message");
        assert_eq!(payload["parse_mode"], "HTML");
        assert_eq!(payload["disable_web_page_preview"], true);
        assert!(payload.get("message_thread_id").is_none());
    }

    #[test]
    fn test_payload_with_thread() {
        let mut payload = json!({
            "chat_id": "-1001234567890",
            "text": "Test message",
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });
        let thread_id = 123_456;
        payload["message_thread_id"] = json!(thread_id);

        assert_eq!(payload["message_thread_id"], 123_456);
    }

    #[test]
    fn test_multiple_clients() {
        let client1 = TelegramClient::new("token1".to_string());
        let client2 = TelegramClient::new("token2".to_string());

        assert_eq!(client1.bot_token, "token1");
        assert_eq!(client2.bot_token, "token2");
        assert_ne!(client1.bot_token, client2.bot_token);
    }
}
