//! Notification system for the liquidator bot.
//!
//! Sends alerts to Telegram when significant events occur:
//! - Successful liquidations
//! - Failed or skipped swaps (unsupported assets, errors)
//!
//! All methods are fire-and-forget: notification failures are logged
//! but never block liquidation operations.

use std::sync::Arc;

use near_sdk::serde_json::json;
use reqwest::Client;

/// Telegram notification configuration.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
    pub thread_id: Option<i64>,
}

/// Shared notifier handle.
pub type SharedNotifier = Arc<Notifier>;

/// Liquidator event notifier.
///
/// When Telegram is configured, sends HTML-formatted messages.
/// When unconfigured, all methods are silent no-ops.
#[derive(Debug)]
pub struct Notifier {
    telegram: Option<TelegramConfig>,
    client: Client,
}

impl Notifier {
    /// Creates a new notifier. Pass `None` to disable notifications.
    pub fn new(telegram: Option<TelegramConfig>) -> Self {
        Self {
            telegram,
            client: Client::new(),
        }
    }

    /// Returns `true` if Telegram notifications are enabled.
    pub fn is_enabled(&self) -> bool {
        self.telegram.is_some()
    }

    /// Notify about a successful liquidation.
    #[allow(clippy::too_many_arguments)]
    pub async fn notify_liquidation(
        &self,
        market: &str,
        borrower: &str,
        send_amount: &str,
        receive_amount: &str,
        profit: &str,
        tx_hash: Option<&str>,
        dry_run: bool,
    ) {
        let prefix = if dry_run { "🧪 DRY RUN " } else { "" };
        let tx_line = tx_hash.map_or(String::new(), |h| format!("\nTx: <code>{h}</code>"));

        let message = format!(
            "{prefix}✅ <b>Liquidation Executed</b>\n\
             \n\
             Market: <code>{market}</code>\n\
             Borrower: <code>{borrower}</code>\n\
             Sent: {send_amount}\n\
             Received: {receive_amount}\n\
             Profit: {profit}{tx_line}"
        );

        self.send(&message).await;
    }

    /// Notify about a failed liquidation attempt.
    pub async fn notify_liquidation_failed(&self, market: &str, borrower: &str, error: &str) {
        let message = format!(
            "❌ <b>Liquidation Failed</b>\n\
             \n\
             Market: <code>{market}</code>\n\
             Borrower: <code>{borrower}</code>\n\
             Error: {error}"
        );

        self.send(&message).await;
    }

    /// Notify about a swap failure after liquidation.
    pub async fn notify_swap_failed(
        &self,
        market: &str,
        from_asset: &str,
        to_asset: &str,
        amount: &str,
        error: &str,
    ) {
        let message = format!(
            "⚠️ <b>Swap Failed</b>\n\
             \n\
             Market: <code>{market}</code>\n\
             From: <code>{from_asset}</code>\n\
             To: <code>{to_asset}</code>\n\
             Amount: {amount}\n\
             Error: {error}"
        );

        self.send(&message).await;
    }

    /// Notify when a swap is skipped because the asset pair is unsupported.
    pub async fn notify_swap_unsupported(
        &self,
        market: &str,
        from_asset: &str,
        to_asset: &str,
        amount: &str,
    ) {
        let message = format!(
            "🚫 <b>Swap Unsupported</b>\n\
             \n\
             Market: <code>{market}</code>\n\
             From: <code>{from_asset}</code>\n\
             To: <code>{to_asset}</code>\n\
             Amount: {amount}\n\
             \n\
             Asset pair not supported by swap provider."
        );

        self.send(&message).await;
    }

    // ── Internal ────────────────────────────────────────────────────────────

    /// Sends an HTML message to the configured Telegram chat.
    /// Failures are logged and swallowed — never propagated.
    async fn send(&self, text: &str) {
        let Some(config) = &self.telegram else {
            return;
        };

        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            config.bot_token
        );

        let mut payload = json!({
            "chat_id": config.chat_id,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });

        if let Some(tid) = config.thread_id {
            payload["message_thread_id"] = json!(tid);
        }

        match self
            .client
            .post(&url)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(response) if response.status() == 429 => {
                tracing::warn!("Telegram rate limit hit, skipping notification");
            }
            Ok(response) if !response.status().is_success() => {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown".to_string());
                tracing::warn!(
                    status = %status,
                    body = %body,
                    "Telegram notification failed"
                );
            }
            Ok(_) => {
                tracing::debug!("Telegram notification sent");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to send Telegram notification");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notifier_disabled_by_default() {
        let notifier = Notifier::new(None);
        assert!(!notifier.is_enabled());
    }

    #[test]
    fn test_notifier_enabled_with_config() {
        let config = TelegramConfig {
            bot_token: "123:ABC".to_string(),
            chat_id: "-100123".to_string(),
            thread_id: None,
        };
        let notifier = Notifier::new(Some(config));
        assert!(notifier.is_enabled());
    }

    #[test]
    fn test_notifier_with_thread_id() {
        let config = TelegramConfig {
            bot_token: "123:ABC".to_string(),
            chat_id: "-100123".to_string(),
            thread_id: Some(42),
        };
        let notifier = Notifier::new(Some(config.clone()));
        assert!(notifier.is_enabled());
        assert_eq!(config.thread_id, Some(42));
    }
}
