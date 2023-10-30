use crate::backup::RemoteBackupClient;
use bitcoin::hashes::hex::ToHex;
use bitcoin::secp256k1::SecretKey;
use bitcoin::secp256k1::SECP256K1;
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
        let client = RemoteBackupClient::new(secret_key.public_key(SECP256K1));

        TenTenOneNodeStorage {
            ln_storage,
            dlc_storage,
            data_dir,
            backup_dir,
            network,
            client,
        }
    }
}

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
}

impl DLCStoreProvider for TenTenOneNodeStorage {
    fn read(&self, keys: Vec<String>) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        self.dlc_storage.read(keys)
    }

    fn write(&self, keys: Vec<String>, value: Vec<u8>) -> anyhow::Result<()> {
        self.dlc_storage.write(keys.clone(), value.clone())?;

        tracing::trace!(
            "Creating a backup of {:?} with value {}",
            keys,
            value.to_hex()
        );

        let key = keys
            .into_iter()
            .filter(|key| !key.is_empty())
            .collect::<Vec<String>>()
            .join("/");
        let key = Path::new("dlc").join(key).to_string_lossy().to_string();

        self.client.backup(key, value);

        Ok(())
    }

    fn delete(&self, keys: Vec<String>) -> anyhow::Result<()> {
        self.dlc_storage.delete(keys.clone())?;

        let key = keys
            .into_iter()
            .filter(|key| !key.is_empty())
            .collect::<Vec<String>>()
            .join("/");

        let key = Path::new("dlc").join(key).to_string_lossy().to_string();

        self.client.delete(key);
        Ok(())
    }
}

impl KVStorePersister for TenTenOneNodeStorage {
    fn persist<W: Writeable>(&self, key: &str, value: &W) -> std::io::Result<()> {
        self.ln_storage.persist(key, value)?;

        let value = value.encode();
        tracing::trace!(
            "Creating a backup of {:?} with value {}",
            key,
            value.to_hex()
        );

        let key = Path::new("ln").join(key).to_string_lossy().to_string();

        self.client.backup(key, value);

        Ok(())
    }
}
