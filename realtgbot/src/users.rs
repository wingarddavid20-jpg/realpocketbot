//! Multi-user storage manager for encrypted user wallets.
//!
//! Stores user profiles in `users.json`. Only encrypted private keys are stored.
//! Plaintext keys are never persisted or exposed.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::error::{BotError, BotResult};

/// Profile of a registered Telegram user.
///
/// Notice: `encrypted_privkey` is ciphertext only. The plain key is never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredUser {
    pub telegram_id: u64,
    pub username: Option<String>,
    pub address: String,
    /// Account ID ("id" from indexer), used for placing orders and querying positions/balance.
    pub account_id: String,
    /// On-chain trading account object ID ("account_object_id" from indexer), kept for balance/deposit reads.
    #[serde(default)]
    pub trading_account_object_id: Option<String>,
    /// Indexer account ID alias (kept for backwards compatibility).
    #[serde(default)]
    pub indexer_account_id: Option<String>,
    pub encrypted_privkey: String,
    pub connected_at: i64,
}

/// Thread-safe in-memory cache backed by a JSON file on disk.
pub struct UserStore {
    file_path: PathBuf,
    users: RwLock<HashMap<u64, StoredUser>>,
}

impl UserStore {
    /// Load users from disk or create an empty store.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let file_path = path.as_ref().to_path_buf();
        let users = if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(content) => match serde_json::from_str::<HashMap<u64, StoredUser>>(&content) {
                    Ok(mut map) => {
                        let mut modified = false;
                        for user in map.values_mut() {
                            // If user was saved under the old bug where account_id held the Trading Account Object ID
                            // and indexer_account_id held the Account ID:
                            if user.trading_account_object_id.is_none() {
                                if let Some(ref idx_id) = user.indexer_account_id {
                                    if !idx_id.is_empty() && idx_id != &user.account_id {
                                        user.trading_account_object_id = Some(user.account_id.clone());
                                        user.account_id = idx_id.clone();
                                        modified = true;
                                    }
                                }
                            }
                        }
                        if modified {
                            if let Ok(data) = serde_json::to_string_pretty(&map) {
                                let _ = std::fs::write(&file_path, data);
                                info!("Migrated existing users in storage to use Account ID as primary account_id");
                            }
                        }
                        info!(count = map.len(), "Loaded users from storage");
                        map
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to parse users storage file; starting empty");
                        HashMap::new()
                    }
                },
                Err(e) => {
                    error!(error = %e, "Failed to read users storage file; starting empty");
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        Self {
            file_path,
            users: RwLock::new(users),
        }
    }

    /// Retrieve a user by their Telegram ID.
    pub async fn get_user(&self, telegram_id: u64) -> Option<StoredUser> {
        let guard = self.users.read().await;
        guard.get(&telegram_id).cloned()
    }

    /// Insert or update a user and flush to disk.
    pub async fn save_user(&self, user: StoredUser) -> BotResult<()> {
        let mut guard = self.users.write().await;
        guard.insert(user.telegram_id, user);

        let data = serde_json::to_string_pretty(&*guard)
            .map_err(|e| BotError::Internal(format!("Failed to serialize users: {e}")))?;

        if let Some(parent) = self.file_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        tokio::fs::write(&self.file_path, data)
            .await
            .map_err(|e| BotError::Internal(format!("Failed to write users file: {e}")))?;

        Ok(())
    }

    /// Remove a user from memory and disk.
    pub async fn remove_user(&self, telegram_id: u64) -> BotResult<bool> {
        let mut guard = self.users.write().await;
        let existed = guard.remove(&telegram_id).is_some();
        if existed {
            let data = serde_json::to_string_pretty(&*guard)
                .map_err(|e| BotError::Internal(format!("Failed to serialize users: {e}")))?;
            tokio::fs::write(&self.file_path, data)
                .await
                .map_err(|e| BotError::Internal(format!("Failed to write users file: {e}")))?;
        }
        Ok(existed)
    }

    /// Total number of registered users.
    pub async fn count(&self) -> usize {
        let guard = self.users.read().await;
        guard.len()
    }

    /// Retrieve all registered user records.
    pub async fn get_all_users(&self) -> Vec<StoredUser> {
        let guard = self.users.read().await;
        guard.values().cloned().collect()
    }
}

pub type SharedUserStore = Arc<UserStore>;
