use bitcoin::hashes::hex::FromHex;
use bitcoin::BlockHash;
use bitcoin::Txid;
use lightning::chain::channelmonitor::ChannelMonitor;
use lightning::sign::EntropySource;
use lightning::sign::SignerProvider;
use lightning::util::persist::KVStorePersister;
use lightning::util::ser::ReadableArgs;
use lightning::util::ser::Writeable;
use ln_dlc_storage::sled::InMemoryDLCStoreProvider;
use ln_dlc_storage::DLCStoreProvider;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Cursor;
use std::ops::Deref;
use std::sync::Arc;

pub trait TenTenOneStorage:
    KVStorePersister + LDKStoreReader + DLCStoreProvider + Sync + Send + Clone
{
}

pub trait BackupClient {
    // delete a backup by key
    fn delete(&self, key: String);

    // backup a value by key
    fn backup(&self, key: String, value: Vec<u8>);

    // restore from a backup
    fn restore(&self) -> anyhow::Result<()>;
}

pub trait LDKStoreReader {
    fn read_network_graph(&self) -> Option<Vec<u8>>;
    fn read_manager(&self) -> Option<Vec<u8>>;
    #[allow(clippy::type_complexity)]
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
        SP::Target: SignerProvider + Sized;
}

#[derive(Clone)]
pub struct TenTenOneInMemoryStorage {
    cache: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    dlc_store: InMemoryDLCStoreProvider,
}

impl TenTenOneInMemoryStorage {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            dlc_store: InMemoryDLCStoreProvider::new(),
        }
    }
}

impl Default for TenTenOneInMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl KVStorePersister for TenTenOneInMemoryStorage {
    fn persist<W: Writeable>(&self, key: &str, object: &W) -> std::io::Result<()> {
        self.cache.lock().insert(key.to_string(), object.encode());
        Ok(())
    }
}

impl LDKStoreReader for TenTenOneInMemoryStorage {
    fn read_network_graph(&self) -> Option<Vec<u8>> {
        self.cache.lock().get("network_graph").cloned()
    }

    fn read_manager(&self) -> Option<Vec<u8>> {
        self.cache.lock().get("manager").cloned()
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
        let mut res = Vec::new();
        for entry in self.cache.lock().iter() {
            if !entry.0.contains("monitors") {
                continue;
            }

            let filename = entry.0;
            if !filename.is_ascii() || filename.len() < 65 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid ChannelMonitor file name",
                ));
            }

            let txid: Txid = Txid::from_hex(filename.split_at(64).0).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid tx ID in filename")
            })?;

            let index: u16 = filename.split_at(65).1.parse().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid tx index in filename",
                )
            })?;

            let contents = entry.1;
            let mut buffer = Cursor::new(&contents);
            match <(
                BlockHash,
                ChannelMonitor<<SP::Target as SignerProvider>::Signer>,
            )>::read(&mut buffer, (&*entropy_source, &*signer_provider))
            {
                Ok((blockhash, channel_monitor)) => {
                    if channel_monitor.get_original_funding_txo().0.txid != txid
                        || channel_monitor.get_original_funding_txo().0.index != index
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "ChannelMonitor was stored in the wrong file",
                        ));
                    }
                    res.push((blockhash, channel_monitor));
                }
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Failed to deserialize ChannelMonitor: {}", e),
                    ))
                }
            }
        }
        Ok(res)
    }
}

impl DLCStoreProvider for TenTenOneInMemoryStorage {
    fn read(&self, keys: Vec<String>) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        self.dlc_store.read(keys)
    }

    fn write(&self, keys: Vec<String>, value: Vec<u8>) -> anyhow::Result<()> {
        self.dlc_store.write(keys, value)
    }

    fn delete(&self, keys: Vec<String>) -> anyhow::Result<()> {
        self.dlc_store.delete(keys)
    }
}

impl TenTenOneStorage for TenTenOneInMemoryStorage {}
