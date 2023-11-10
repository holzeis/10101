use crate::cipher::AesCipher;
use crate::config;
use crate::db;
use crate::event::subscriber::Subscriber;
use crate::event::EventInternal;
use crate::event::EventType;
use anyhow::bail;
use anyhow::ensure;
use anyhow::Result;
use coordinator_commons::Backup;
use coordinator_commons::DeleteBackup;
use coordinator_commons::Restore;
use coordinator_commons::RestoreKind;
use futures::future::RemoteHandle;
use futures::FutureExt;
use ln_dlc_storage::sled::SledStorageProvider;
use ln_dlc_storage::DLCStoreProvider;
use reqwest::Client;
use reqwest::StatusCode;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::spawn_blocking;

const BLACKLIST: [&str; 1] = ["ln/network_graph"];

#[derive(Clone)]
pub struct DBBackupSubscriber {
    client: RemoteBackupClient,
}

impl DBBackupSubscriber {
    pub fn new(client: RemoteBackupClient) -> Self {
        Self { client }
    }

    pub fn backup(&self) -> Result<()> {
        let db_backup = db::backup()?;
        tracing::debug!("Successfully created backup of database! Uploading snapshot!");
        let value = fs::read(db_backup)?;
        spawn_blocking({
            let client = self.client.clone();
            move || client.backup("10101/db".to_string(), value).forget()
        });

        Ok(())
    }
}

impl Subscriber for DBBackupSubscriber {
    fn notify(&self, _event: &EventInternal) {
        if let Err(e) = self.backup() {
            tracing::error!("Failed to backup db. {e:#}");
        }
    }

    fn events(&self) -> Vec<EventType> {
        vec![
            EventType::PaymentClaimed,
            EventType::PaymentSent,
            EventType::PaymentFailed,
            EventType::PositionUpdateNotification,
            EventType::PositionClosedNotification,
            EventType::OrderUpdateNotification,
            EventType::OrderFilledWith,
            EventType::SpendableOutputs,
        ]
    }
}

#[derive(Clone)]
pub struct RemoteBackupClient {
    inner: Client,
    endpoint: String,
    cipher: AesCipher,
}

impl RemoteBackupClient {
    pub fn new(cipher: AesCipher) -> RemoteBackupClient {
        let inner = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Could not build reqwest client");

        Self {
            inner,
            endpoint: format!("http://{}/api", config::get_http_endpoint()),
            cipher,
        }
    }
}

impl RemoteBackupClient {
    pub fn delete(&self, key: String) -> RemoteHandle<()> {
        let (fut, remote_handle) = {
            let client = self.inner.clone();
            let node_id = self.cipher.public_key();
            let endpoint = format!("{}/backup/{}", self.endpoint.clone(), node_id);
            let cipher = self.cipher.clone();
            let message = node_id.to_string().as_bytes().to_vec();
            async move {
                let signature = match cipher.sign(message) {
                    Ok(signature) => signature,
                    Err(e) => {
                        tracing::error!(%key, "{e:#}");
                        return;
                    }
                };

                let backup = DeleteBackup {
                    key: key.clone(),
                    signature,
                };

                if let Err(e) = client.delete(endpoint).json(&backup).send().await {
                    tracing::error!("Failed to delete backup of {key}. {e:#}")
                } else {
                    tracing::debug!("Successfully deleted backup of {key}");
                }
            }
        }
        .remote_handle();

        tokio::spawn(fut);

        remote_handle
    }

    pub fn backup(&self, key: String, value: Vec<u8>) -> RemoteHandle<()> {
        tracing::trace!("Creating backup for {key}");
        let (fut, remote_handle) = {
            let client = self.inner.clone();
            let cipher = self.cipher.clone();
            let node_id = cipher.public_key();
            let endpoint = format!("{}/backup/{}", self.endpoint.clone(), node_id);
            async move {
                if BLACKLIST.contains(&key.as_str()) {
                    tracing::debug!(key, "Skipping blacklisted backup");
                    return;
                }

                let encrypted_value = match cipher.encrypt(value) {
                    Ok(encrypted_value) => encrypted_value,
                    Err(e) => {
                        tracing::error!(%key, "{e:#}");
                        return;
                    }
                };
                let signature = match cipher.sign(encrypted_value.clone()) {
                    Ok(signature) => signature,
                    Err(e) => {
                        tracing::error!(%key, "{e:#}");
                        return;
                    }
                };

                let backup = Backup {
                    key: key.clone(),
                    value: encrypted_value,
                    signature,
                };

                match client.post(endpoint).json(&backup).send().await {
                    Ok(response) => {
                        tracing::debug!("Response status code {}", response.status());
                        if response.status() != StatusCode::OK {
                            match response.text().await {
                                Ok(response) => {
                                    tracing::error!("Failed to upload backup. {response}")
                                }
                                Err(e) => tracing::error!("Failed to upload backup. {e}"),
                            }
                        } else {
                            tracing::debug!("Successfully uploaded backup of {key}.");
                        }
                    }
                    Err(e) => tracing::error!("Failed to create a backup of {key}. {e:#}"),
                }
            }
        }
        .remote_handle();

        tokio::spawn(fut);

        remote_handle
    }

    pub async fn restore(&self, dlc_storage: Arc<SledStorageProvider>) -> Result<()> {
        tokio::spawn({
            let client = self.inner.clone();
            let cipher = self.cipher.clone();
            let node_id = cipher.public_key();
            let endpoint = format!("{}/restore/{}", self.endpoint.clone(), node_id);
            let data_dir = config::get_data_dir();
            let network = config::get_network();
            let message = node_id.to_string().as_bytes().to_vec();
            async move {
                let signature = cipher.sign(message)?;

                match client.get(endpoint).json(&signature).send().await {
                    Ok(response) => {
                        tracing::debug!("Response status code {}", response.status());
                        if response.status() != StatusCode::OK {
                            let response = response.text().await?;
                            bail!("Failed to download backup. {response}");
                        }

                        let backup: Vec<Restore> = response.json().await?;
                        tracing::debug!("Successfully downloaded backup.");

                        for restore in backup.into_iter() {
                            let decrypted_value = cipher.decrypt(restore.value)?;
                            match restore.kind {
                                RestoreKind::LN => {
                                    tracing::debug!("Restoring {}", restore.key);
                                    let dest_file = Path::new(&data_dir)
                                        .join(network.to_string())
                                        .join(restore.key.clone());

                                    fs::create_dir_all(dest_file.parent().expect("parent"))?;
                                    fs::write(dest_file.as_path(), decrypted_value)?;
                                }
                                RestoreKind::DLC => {
                                    tracing::debug!("Restoring {}", restore.key);
                                    let keys = restore.key.split('/').collect::<Vec<&str>>();
                                    ensure!(keys.len() == 2, "dlc key is too short");

                                    let kind = *hex::decode(keys.first().expect("to exist"))?
                                        .first()
                                        .expect("to exist");

                                    let key = hex::decode(keys.get(1).expect("to exist"))?;

                                    dlc_storage.write(kind, key, decrypted_value)?;
                                }
                                RestoreKind::TenTenOne => {
                                    let data_dir = Path::new(&data_dir);
                                    let db_file =
                                        data_dir.join(format!("trades-{}.sqlite", network));
                                    tracing::debug!(
                                        "Restoring 10101 database backup into {}",
                                        db_file.to_string_lossy().to_string()
                                    );
                                    fs::write(db_file.as_path(), decrypted_value)?;
                                }
                            }
                        }
                        tracing::info!("Successfully restored 10101 from backup!");
                    }
                    Err(e) => bail!("Failed to download backup. {e:#}"),
                }
                Ok(())
            }
        })
        .await?
    }
}
