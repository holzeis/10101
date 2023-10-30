use crate::config;
use crate::db;
use crate::event::subscriber::Subscriber;
use crate::event::EventInternal;
use crate::event::EventType;
use anyhow::bail;
use bitcoin::hashes::hex::ToHex;
use bitcoin::secp256k1::PublicKey;
use coordinator_commons::Restore;
use coordinator_commons::RestoreKind;
use ln_dlc_storage::sled::SledStorageProvider;
use ln_dlc_storage::DLCStoreProvider;
use reqwest::Body;
use reqwest::Client;
use reqwest::StatusCode;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::spawn_blocking;
use tokio_util::codec::BytesCodec;
use tokio_util::codec::FramedRead;

#[derive(Clone)]
pub struct DBBackupSubscriber {
    client: RemoteBackupClient,
}

impl DBBackupSubscriber {
    pub fn new(client: RemoteBackupClient) -> Self {
        Self { client }
    }
}

impl Subscriber for DBBackupSubscriber {
    fn notify(&self, _event: &EventInternal) {
        let db_backup = match db::backup() {
            Ok(db_backup) => {
                tracing::debug!("Successfully created backup of database! Uploading snapshot!");
                db_backup
            }
            Err(error) => {
                tracing::error!("Failed to create backup of database. {error}");
                return;
            }
        };

        match fs::read(db_backup) {
            Ok(value) => {
                spawn_blocking({
                    let client = self.client.clone();
                    move || client.backup("10101/db".to_string(), value)
                });
            }
            Err(e) => tracing::error!("Failed to create backup of database. {e:#}"),
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
        ]
    }
}

#[derive(Clone)]
pub struct RemoteBackupClient {
    client: Client,
    backup_endpoint: String,
    restore_endpoint: String,
}

impl RemoteBackupClient {
    pub fn new(node_id: PublicKey) -> RemoteBackupClient {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Could not build reqwest client");

        let backup_endpoint = format!(
            "http://{}/api/backup/{}",
            config::get_http_endpoint(),
            node_id
        );

        let restore_endpoint = format!(
            "http://{}/api/restore/{}",
            config::get_http_endpoint(),
            node_id
        );

        Self {
            client,
            backup_endpoint,
            restore_endpoint,
        }
    }
}

impl RemoteBackupClient {
    pub fn delete(&self, key: String) {
        tokio::spawn({
            let client = self.client.clone();
            let endpoint = format!("{}/{key}", self.backup_endpoint.clone());
            async move {
                if let Err(e) = client.delete(endpoint).send().await {
                    tracing::error!("Failed to delete backup of {key}. {e:#}")
                } else {
                    tracing::debug!("Successfully deleted backup of {key}");
                }
            }
        });
    }

    pub fn backup(&self, key: String, value: Vec<u8>) {
        tokio::spawn({
            let client = self.client.clone();
            let endpoint = format!("{}/{key}", self.backup_endpoint.clone());
            async move {
                let cursor = Cursor::new(value);
                let framed_read = FramedRead::new(cursor, BytesCodec::new());
                let body = Body::wrap_stream(framed_read);
                match client.post(endpoint).body(body).send().await {
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
                            let resp = response.text().await.expect("response");
                            tracing::debug!(
                                "Successfully uploaded backup of {key}. Response: {resp}"
                            );
                        }
                    }
                    Err(e) => tracing::error!("Failed to create a backup of {key}. {e:#}"),
                }
            }
        });
    }

    pub async fn restore(&self, dlc_storage: Arc<SledStorageProvider>) -> anyhow::Result<()> {
        tokio::spawn({
            let client = self.client.clone();
            let endpoint = self.restore_endpoint.clone();
            let data_dir = config::get_data_dir();
            let network = config::get_network();
            async move {
                match client.get(endpoint).send().await {
                    Ok(response) => {
                        tracing::debug!("Response status code {}", response.status());
                        if response.status() != StatusCode::OK {
                            let response = response.text().await?;
                            bail!("Failed to download backup. {response}");
                        }

                        let backup: Vec<Restore> = response.json().await?;
                        tracing::debug!("Successfully downloaded backup.");

                        for restore in backup.into_iter() {
                            match restore.kind {
                                RestoreKind::LN => {
                                    tracing::debug!(
                                        "Restoring {} with value {}",
                                        restore.key,
                                        restore.value.to_hex()
                                    );
                                    let dest_file = Path::new(&data_dir)
                                        .join(network.to_string())
                                        .join(restore.key.clone());

                                    fs::create_dir_all(dest_file.parent().expect("parent"))?;
                                    fs::write(dest_file.as_path(), &restore.value)?;
                                }
                                RestoreKind::DLC => {
                                    tracing::debug!(
                                        "Restoring {} with value {}",
                                        restore.key,
                                        restore.value.to_hex()
                                    );
                                    let keys =
                                        restore.key.split('/').map(|k| k.to_string()).collect();
                                    dlc_storage.write(keys, restore.value)?;
                                }
                                RestoreKind::TenTenOne => {
                                    let data_dir = Path::new(&data_dir);
                                    let db_file =
                                        data_dir.join(format!("trades-{}.sqlite", network));
                                    tracing::debug!(
                                        "Restoring 10101 database backup into {}",
                                        db_file.to_string_lossy().to_string()
                                    );
                                    fs::write(db_file.as_path(), restore.value)?;
                                }
                            }
                        }
                        tracing::info!("Successfully restored 10101 from backup!");
                    }
                    Err(e) => bail!("Failed to download backup. {e:#}"),
                }
                anyhow::Ok(())
            }
        })
        .await?
    }
}
