//! Notification system for the liquidator bot.
//!
//! Sends alerts to Telegram when significant events occur:
//! - Successful liquidations
//! - Failed or skipped swaps (unsupported assets, errors)
//!
//! All `notify_*` methods are truly fire-and-forget: they spawn the HTTP
//! request on a background task and return immediately, so they never
//! block liquidation operations.

use std::sync::Arc;

use near_sdk::serde_json::json;
use reqwest::Client;

/// A string wrapper that redacts its value in Debug output.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    /// Access the inner value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Telegram notification configuration.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub bot_token: SecretString,
    pub chat_id: String,
    pub thread_id: Option<i64>,
}

/// Shared notifier handle.
pub type SharedNotifier = Arc<Notifier>;

/// Liquidator event notifier.
///
/// When Telegram is configured, sends HTML-formatted messages via
/// background tasks. When unconfigured, all methods are silent no-ops.
#[derive(Debug)]
pub struct Notifier {
    telegram: Option<TelegramConfig>,
    client: Client,
}

/// Escape HTML special characters in dynamic values so they don't break
/// Telegram's HTML parse mode or get rejected.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ── Message formatting (pure functions, easily testable) ────────────────

/// Format a successful liquidation message.
#[allow(clippy::too_many_arguments)]
fn format_liquidation_message(
    market: &str,
    borrower: &str,
    send_amount: &str,
    receive_amount: &str,
    profit: &str,
    tx_hash: Option<&str>,
    dry_run: bool,
) -> String {
    let prefix = if dry_run { "🧪 DRY RUN " } else { "" };
    let tx_line = tx_hash.map_or(String::new(), |h| {
        format!("\nTx: <code>{}</code>", html_escape(h))
    });

    format!(
        "{prefix}✅ <b>Liquidation Executed</b>\n\
         \n\
         Market: <code>{}</code>\n\
         Borrower: <code>{}</code>\n\
         Sent: {}\n\
         Received: {}\n\
         Profit: {}{tx_line}",
        html_escape(market),
        html_escape(borrower),
        html_escape(send_amount),
        html_escape(receive_amount),
        html_escape(profit),
    )
}

/// Format a failed liquidation message.
fn format_liquidation_failed_message(market: &str, borrower: &str, error: &str) -> String {
    format!(
        "❌ <b>Liquidation Failed</b>\n\
         \n\
         Market: <code>{}</code>\n\
         Borrower: <code>{}</code>\n\
         Error: {}",
        html_escape(market),
        html_escape(borrower),
        html_escape(error),
    )
}

/// Format a swap failure message.
fn format_swap_failed_message(
    market: &str,
    from_asset: &str,
    to_asset: &str,
    amount: &str,
    error: &str,
) -> String {
    format!(
        "⚠️ <b>Swap Failed</b>\n\
         \n\
         Market: <code>{}</code>\n\
         From: <code>{}</code>\n\
         To: <code>{}</code>\n\
         Amount: {}\n\
         Error: {}",
        html_escape(market),
        html_escape(from_asset),
        html_escape(to_asset),
        html_escape(amount),
        html_escape(error),
    )
}

/// Format a swap-unsupported message.
fn format_swap_unsupported_message(
    market: &str,
    from_asset: &str,
    to_asset: &str,
    amount: &str,
) -> String {
    format!(
        "🚫 <b>Swap Unsupported</b>\n\
         \n\
         Market: <code>{}</code>\n\
         From: <code>{}</code>\n\
         To: <code>{}</code>\n\
         Amount: {}\n\
         \n\
         Asset pair not supported by swap provider.",
        html_escape(market),
        html_escape(from_asset),
        html_escape(to_asset),
        html_escape(amount),
    )
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
    pub fn notify_liquidation(
        self: &Arc<Self>,
        market: &str,
        borrower: &str,
        send_amount: &str,
        receive_amount: &str,
        profit: &str,
        tx_hash: Option<&str>,
        dry_run: bool,
    ) {
        self.spawn_send(format_liquidation_message(
            market,
            borrower,
            send_amount,
            receive_amount,
            profit,
            tx_hash,
            dry_run,
        ));
    }

    /// Notify about a failed liquidation attempt.
    pub fn notify_liquidation_failed(self: &Arc<Self>, market: &str, borrower: &str, error: &str) {
        self.spawn_send(format_liquidation_failed_message(market, borrower, error));
    }

    /// Notify about a swap failure after liquidation.
    pub fn notify_swap_failed(
        self: &Arc<Self>,
        market: &str,
        from_asset: &str,
        to_asset: &str,
        amount: &str,
        error: &str,
    ) {
        self.spawn_send(format_swap_failed_message(
            market, from_asset, to_asset, amount, error,
        ));
    }

    /// Notify when a swap is skipped because the asset pair is unsupported.
    pub fn notify_swap_unsupported(
        self: &Arc<Self>,
        market: &str,
        from_asset: &str,
        to_asset: &str,
        amount: &str,
    ) {
        self.spawn_send(format_swap_unsupported_message(
            market, from_asset, to_asset, amount,
        ));
    }

    // ── Internal ────────────────────────────────────────────────────────────

    /// Spawns the send on a background task so callers never block.
    fn spawn_send(self: &Arc<Self>, message: String) {
        if self.telegram.is_none() {
            return;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.send(&message).await;
        });
    }

    /// Sends an HTML message to the configured Telegram chat.
    /// Failures are logged and swallowed — never propagated.
    async fn send(&self, text: &str) {
        let Some(config) = &self.telegram else {
            return;
        };

        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            config.bot_token.as_str()
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
                let safe_error = e.without_url();
                tracing::warn!(error = %safe_error, "Failed to send Telegram notification");
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
            bot_token: "123:ABC".to_string().into(),
            chat_id: "-100123".to_string(),
            thread_id: None,
        };
        let notifier = Notifier::new(Some(config));
        assert!(notifier.is_enabled());
    }

    #[test]
    fn test_notifier_with_thread_id() {
        let config = TelegramConfig {
            bot_token: "123:ABC".to_string().into(),
            chat_id: "-100123".to_string(),
            thread_id: Some(42),
        };
        let notifier = Notifier::new(Some(config.clone()));
        assert!(notifier.is_enabled());
        assert_eq!(config.thread_id, Some(42));
    }

    #[test]
    fn test_secret_string_redacts_debug() {
        let secret = SecretString::from("my-secret-token".to_string());
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(secret.as_str(), "my-secret-token");
    }

    #[test]
    fn test_telegram_config_debug_redacts_token() {
        let config = TelegramConfig {
            bot_token: "super-secret".to_string().into(),
            chat_id: "-100123".to_string(),
            thread_id: None,
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
        assert_eq!(html_escape("no special chars"), "no special chars");
        assert_eq!(
            html_escape("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
    }

    #[test]
    fn test_format_liquidation_message() {
        let msg = format_liquidation_message(
            "market.near",
            "borrower.near",
            "100.00 USDC",
            "0.005 BTC",
            "+1.50 USDC (+1.5%)",
            None,
            false,
        );
        assert!(msg.contains("✅ <b>Liquidation Executed</b>"));
        assert!(msg.contains("<code>market.near</code>"));
        assert!(msg.contains("<code>borrower.near</code>"));
        assert!(msg.contains("100.00 USDC"));
        assert!(msg.contains("0.005 BTC"));
        assert!(msg.contains("+1.50 USDC (+1.5%)"));
        assert!(!msg.contains("Tx:"));
    }

    #[test]
    fn test_format_liquidation_message_dry_run() {
        let msg = format_liquidation_message(
            "market.near",
            "borrower.near",
            "100.00 USDC",
            "0.005 BTC",
            "+1.50 USDC",
            None,
            true,
        );
        assert!(msg.starts_with("🧪 DRY RUN ✅"));
    }

    #[test]
    fn test_format_liquidation_message_with_tx_hash() {
        let msg = format_liquidation_message(
            "market.near",
            "borrower.near",
            "100.00 USDC",
            "0.005 BTC",
            "+1.50 USDC",
            Some("abc123"),
            false,
        );
        assert!(msg.contains("Tx: <code>abc123</code>"));
    }

    #[test]
    fn test_format_liquidation_message_escapes_html() {
        let msg = format_liquidation_message(
            "a<b>c", "x&y", "10 USDC", "0.1 BTC", "+1 USDC", None, false,
        );
        assert!(msg.contains("a&lt;b&gt;c"));
        assert!(msg.contains("x&amp;y"));
    }

    #[test]
    fn test_format_liquidation_failed_message() {
        let msg = format_liquidation_failed_message(
            "market.near",
            "borrower.near",
            "Transaction timed out",
        );
        assert!(msg.contains("❌ <b>Liquidation Failed</b>"));
        assert!(msg.contains("<code>market.near</code>"));
        assert!(msg.contains("<code>borrower.near</code>"));
        assert!(msg.contains("Transaction timed out"));
    }

    #[test]
    fn test_format_liquidation_failed_escapes_error() {
        let msg = format_liquidation_failed_message("m", "b", "error <contains> html & stuff");
        assert!(msg.contains("error &lt;contains&gt; html &amp; stuff"));
    }

    #[test]
    fn test_format_swap_failed_message() {
        let msg = format_swap_failed_message("market.near", "BTC", "USDC", "0.005 BTC", "No route");
        assert!(msg.contains("⚠️ <b>Swap Failed</b>"));
        assert!(msg.contains("<code>BTC</code>"));
        assert!(msg.contains("<code>USDC</code>"));
        assert!(msg.contains("0.005 BTC"));
        assert!(msg.contains("No route"));
    }

    #[test]
    fn test_format_swap_unsupported_message() {
        let msg = format_swap_unsupported_message("market.near", "stNEAR", "USDC", "100 stNEAR");
        assert!(msg.contains("🚫 <b>Swap Unsupported</b>"));
        assert!(msg.contains("<code>stNEAR</code>"));
        assert!(msg.contains("<code>USDC</code>"));
        assert!(msg.contains("100 stNEAR"));
        assert!(msg.contains("not supported by swap provider"));
    }

    #[test]
    fn test_spawn_send_noop_when_disabled() {
        let notifier = Arc::new(Notifier::new(None));
        // Should not panic or spawn anything
        notifier.notify_liquidation("m", "b", "1", "2", "3", None, false);
        notifier.notify_liquidation_failed("m", "b", "err");
        notifier.notify_swap_failed("m", "a", "b", "1", "err");
        notifier.notify_swap_unsupported("m", "a", "b", "1");
    }
}
