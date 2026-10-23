//! Price alerts storage and management.
//!
//! Stores active user price alerts in `alerts.json`. When mark prices cross
//! the target threshold, background poller sends notification and marks them triggered.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::error::{BotError, BotResult};

/// An alert requested by a Telegram user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceAlert {
    pub id: String,
    pub telegram_id: u64,
    pub chat_id: i64,
    pub market_id: String,
    pub symbol: String,
    pub target_price: f64,
    /// If true, triggers when current_price >= target_price.
    /// If false, triggers when current_price <= target_price.
    pub trigger_above: bool,
    pub created_at: i64,
    pub triggered: bool,
}

/// Thread-safe in-memory cache backed by a JSON file on disk.
pub struct AlertStore {
    file_path: PathBuf,
    alerts: RwLock<Vec<PriceAlert>>,
}

impl AlertStore {
    /// Load alerts from disk or create an empty store.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let file_path = path.as_ref().to_path_buf();
        let alerts = if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(content) => match serde_json::from_str::<Vec<PriceAlert>>(&content) {
                    Ok(list) => {
                        info!(count = list.len(), "Loaded alerts from storage");
                        list
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to parse alerts file; starting empty");
                        Vec::new()
                    }
                },
                Err(e) => {
                    error!(error = %e, "Failed to read alerts file; starting empty");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        Self {
            file_path,
            alerts: RwLock::new(alerts),
        }
    }

    /// Add a new price alert and save to disk.
    pub async fn add_alert(&self, alert: PriceAlert) -> BotResult<()> {
        let mut guard = self.alerts.write().await;
        guard.push(alert);
        self.save_to_disk(&guard).await
    }

    /// Get all active (untriggered) alerts for a specific user.
    pub async fn get_user_alerts(&self, telegram_id: u64) -> Vec<PriceAlert> {
        let guard = self.alerts.read().await;
        guard
            .iter()
            .filter(|a| a.telegram_id == telegram_id && !a.triggered)
            .cloned()
            .collect()
    }

    /// Remove an alert by its ID (must match caller's telegram_id).
    pub async fn remove_alert(&self, alert_id: &str, telegram_id: u64) -> BotResult<bool> {
        let mut guard = self.alerts.write().await;
        let before_len = guard.len();
        guard.retain(|a| !(a.id == alert_id && a.telegram_id == telegram_id));
        let changed = guard.len() != before_len;
        if changed {
            self.save_to_disk(&guard).await?;
        }
        Ok(changed)
    }

    /// Get all pending active alerts for the background checker.
    pub async fn get_active_alerts(&self) -> Vec<PriceAlert> {
        let guard = self.alerts.read().await;
        guard.iter().filter(|a| !a.triggered).cloned().collect()
    }

    /// Mark an alert as triggered and remove it from active storage.
    pub async fn mark_triggered(&self, alert_id: &str) -> BotResult<()> {
        let mut guard = self.alerts.write().await;
        if let Some(pos) = guard.iter_mut().find(|a| a.id == alert_id) {
            pos.triggered = true;
        }
        // Retain only untriggered alerts to keep file clean
        guard.retain(|a| !a.triggered);
        self.save_to_disk(&guard).await
    }

    async fn save_to_disk(&self, alerts: &[PriceAlert]) -> BotResult<()> {
        let data = serde_json::to_string_pretty(alerts)
            .map_err(|e| BotError::Internal(format!("Failed to serialize alerts: {e}")))?;

        if let Some(parent) = self.file_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        tokio::fs::write(&self.file_path, data)
            .await
            .map_err(|e| BotError::Internal(format!("Failed to write alerts file: {e}")))?;

        Ok(())
    }
}

pub type SharedAlertStore = Arc<AlertStore>;
