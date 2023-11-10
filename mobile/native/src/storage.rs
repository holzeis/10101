use crate::backup::RemoteBackupClient;
use crate::cipher::AesCipher;
use crate::db;
use bitcoin::hashes::hex::ToHex;
use bitcoin::secp256k1::SecretKey;
use bitcoin::BlockHash;
use bitcoin::Network;
use lightning::chain::channelmonitor::ChannelMonitor;
use lightning::sign::EntropySource;
use lightning::sign::SignerProvider;
use lightning::util::persist::KVStorePersister;
use lightning::util::ser::Writeable;
use lightning_persister::FilesystemPersister;
use ln_dlc_node::storage::LDKStoreReader;
use ln_dlc_node::storage::TenTenOneStorage;
use ln_dlc_storage::sled::SledStorageProvider;
use ln_dlc_storage::DLCStoreProvider;
use std::fs;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct TenTenOneNodeStorage {
    pub client: RemoteBackupClient,
    pub ln_storage: Arc<FilesystemPersister>,
    pub dlc_storage: Arc<SledStorageProvider>,
    pub data_dir: String,
    pub backup_dir: String,
    pub network: Network,
}

impl TenTenOneStorage for TenTenOneNodeStorage {}

impl TenTenOneNodeStorage {
    pub fn new(data_dir: String, network: Network, secret_key: SecretKey) -> TenTenOneNodeStorage {
        let mut data_dir = PathBuf::from(data_dir);
        data_dir.push(network.to_string());

        if !data_dir.exists() {
            fs::create_dir_all(data_dir.as_path())
                .unwrap_or_else(|_| panic!("Failed to create data dir {data_dir:?}"));
        }

        let backup_dir = data_dir.join(Path::new("backup"));
        if !backup_dir.exists() {
            fs::create_dir_all(backup_dir.as_path())
                .unwrap_or_else(|_| panic!("Failed to create backup dir {backup_dir:?}"));
        }

        let data_dir = data_dir.to_string_lossy().to_string();
        let backup_dir = backup_dir.to_string_lossy().to_string();
        tracing::info!("Created backup dir at {backup_dir}");

        let ln_storage = Arc::new(FilesystemPersister::new(data_dir.clone()));
        let dlc_storage = Arc::new(SledStorageProvider::new(&data_dir));
        let client = RemoteBackupClient::new(AesCipher::new(secret_key));

        TenTenOneNodeStorage {
            ln_storage,
            dlc_storage,
            data_dir,
            backup_dir,
            network,
            client,
        }
    }

    /// Creates a full backup of the lightning and dlc data.
    pub async fn full_backup(&self) -> anyhow::Result<()> {
        tracing::info!("Running full backup");
        let mut handles = vec![];

        let db_backup = db::backup()?;
        let value = fs::read(db_backup)?;
        let handle = self.client.backup("10101/db".to_string(), value);
        handles.push(handle);

        for ln_backup in self.export()?.into_iter() {
            let handle = self
                .client
                .backup(["ln".to_string(), ln_backup.0].join("/"), ln_backup.1);
            handles.push(handle);
        }

        for dlc_backup in self.dlc_storage.export()?.into_iter() {
            let key = [
                "dlc",
                &hex::encode([dlc_backup.0]),
                &hex::encode(dlc_backup.1),
            ]
            .join("/");
            let handle = self.client.backup(key, dlc_backup.2);
            handles.push(handle);
        }

        futures::future::join_all(handles).await;

        tracing::info!("Successfully created a full backup!");

        Ok(())
    }
}

// TODO(holzeis): This trait should be implemented on the FilesystemPersister. Note, this should be
// done by implementing a TenTenOneFilesystemPersister.
impl LDKStoreReader for TenTenOneNodeStorage {
    fn read_network_graph(&self) -> Option<Vec<u8>> {
        let path = &format!("{}/network_graph", self.data_dir);
        let network_graph_path = Path::new(path);
        network_graph_path
            .exists()
            .then(|| fs::read(network_graph_path).expect("network graph to be readable"))
    }

    fn read_manager(&self) -> Option<Vec<u8>> {
        let path = &format!("{}/manager", self.data_dir);
        let manager_path = Path::new(path);
        manager_path
            .exists()
            .then(|| fs::read(manager_path).expect("manager to be readable"))
    }

    fn read_channelmonitors<ES: Deref, SP: Deref>(
        &self,
        entropy_source: ES,
        signer_provider: SP,
    ) -> std::io::Result<
        Vec<(
            BlockHash,
            ChannelMonitor<<SP::Target as SignerProvider>::Signer>,
        )>,
    >
    where
        ES::Target: EntropySource + Sized,
        SP::Target: SignerProvider + Sized,
    {
        self.ln_storage
            .read_channelmonitors(entropy_source, signer_provider)
    }

    fn export(&self) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let mut export = vec![];
        if let Some(manager) = self.read_manager() {
            export.push(("ln/manager".to_string(), manager));
        }

        let path = &format!("{}/monitors", self.data_dir);
        let monitors = fs::read_dir(path)?;
        for monitor in monitors {
            let monitor = monitor?;
            let value = fs::read(monitor.path())?;
            let key = monitor.file_name().to_string_lossy().to_string();
            export.push((format!("ln/monitors/{key}"), value));
        }

        Ok(export)
    }
}

impl DLCStoreProvider for TenTenOneNodeStorage {
    fn read(&self, kind: u8, key: Option<Vec<u8>>) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.dlc_storage.read(kind, key)
    }

    fn write(&self, kind: u8, key: Vec<u8>, value: Vec<u8>) -> anyhow::Result<()> {
        self.dlc_storage.write(kind, key.clone(), value.clone())?;

        let key = ["dlc", &hex::encode([kind]), &hex::encode(key)].join("/");

        // Let the backup run asynchronously we don't really care if it is successful or not as the
        // next write may fix the issue. Note, if we want to handle failed backup attempts we
        // would need to remember those remote handles and handle a failure accordingly.
        self.client.backup(key, value).forget();

        Ok(())
    }

    fn delete(&self, kind: u8, key: Option<Vec<u8>>) -> anyhow::Result<()> {
        self.dlc_storage.delete(kind, key.clone())?;

        let key = match key {
            Some(key) => ["dlc", &kind.to_hex(), &hex::encode(key)].join("/"),
            None => ["dlc", &hex::encode([kind])].join("/"),
        };

        // Let the backup run asynchronously we don't really care if it is successful or not. We may
        // end up with a key that should have been deleted. That should hopefully not be a problem.
        // Note, if we want to handle failed backup attempts we would need to remember those
        // remote handles and handle a failure accordingly.
        self.client.delete(key).forget();
        Ok(())
    }
}

impl KVStorePersister for TenTenOneNodeStorage {
    fn persist<W: Writeable>(&self, key: &str, value: &W) -> std::io::Result<()> {
        self.ln_storage.persist(key, value)?;

        let value = value.encode();
        tracing::trace!("Creating a backup of {:?}", key);

        // Let the backup run asynchronously we don't really care if it is successful or not as the
        // next persist will fix the issue. Note, if we want to handle failed backup attempts we
        // would need to remember those remote handles and handle a failure accordingly.
        self.client.backup(["ln", key].join("/"), value).forget();

        Ok(())
    }
}
