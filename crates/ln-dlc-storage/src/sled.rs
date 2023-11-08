use crate::DLCStoreProvider;
use anyhow::Context;
use parking_lot::RwLock;
use sled::Db;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct SledStorageProvider {
    db: Db,
}

pub type SledStorageExport = Vec<(u8, Vec<u8>, Vec<u8>)>;

impl SledStorageProvider {
    pub fn new(path: &str) -> Self {
        SledStorageProvider {
            db: sled::open(path).expect("valid path"),
        }
    }

    /// Migrates the old keys of the dlc-sled-storage-provider::SledStorageProvider to the
    /// ln_dlc_crate::SledStorageProvider
    pub fn migrate(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Exports all key value pairs from the sled storage
    pub fn export(&self) -> anyhow::Result<SledStorageExport> {
        let mut export = vec![];
        for (collection_type, collection_name, collection_iter) in self.db.export() {
            if collection_type != b"tree" {
                continue;
            }
            for mut kv in collection_iter {
                let value = kv.pop().expect("failed to get value from tree export");
                let key = kv.pop().expect("failed to get key from tree export");
                let kind = collection_name
                    .first()
                    .expect("failed to get kind from tree export");

                export.push((*kind, key, value));
            }
        }
        Ok(export)
    }
}

impl DLCStoreProvider for SledStorageProvider {
    fn read(&self, kind: u8, key: Option<Vec<u8>>) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let tree = self.db.open_tree([kind])?;

        if let Some(key) = key {
            let result = match tree.get(key.clone())? {
                Some(value) => vec![(key, value.to_vec())],
                None => vec![],
            };

            Ok(result)
        } else {
            let result = tree
                .iter()
                .map(|entry| {
                    let entry = entry.expect("to not fail");
                    (entry.0.to_vec(), entry.1.to_vec())
                })
                .collect();

            Ok(result)
        }
    }

    fn write(&self, kind: u8, key: Vec<u8>, value: Vec<u8>) -> anyhow::Result<()> {
        self.db.open_tree([kind])?.insert(key, value)?;
        self.db.flush()?;
        Ok(())
    }

    fn delete(&self, kind: u8, key: Option<Vec<u8>>) -> anyhow::Result<()> {
        let tree = self.db.open_tree([kind])?;

        if let Some(key) = key {
            tree.remove(key)?;
        } else {
            tree.clear()?;
        }

        self.db.flush()?;
        Ok(())
    }
}

type InMemoryStore = Arc<RwLock<HashMap<u8, HashMap<Vec<u8>, Vec<u8>>>>>;

#[derive(Clone)]
pub struct InMemoryDLCStoreProvider {
    memory: InMemoryStore,
}

impl Default for InMemoryDLCStoreProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryDLCStoreProvider {
    pub fn new() -> Self {
        InMemoryDLCStoreProvider {
            memory: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl DLCStoreProvider for InMemoryDLCStoreProvider {
    fn read(&self, kind: u8, key: Option<Vec<u8>>) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let store = self.memory.read();
        let store = match store.get(&kind) {
            Some(store) => store,
            None => return Ok(vec![]),
        };

        if let Some(key) = key {
            let result = match store.get(&key) {
                Some(value) => vec![(key, value.clone())],
                None => vec![],
            };
            Ok(result)
        } else {
            Ok(store.clone().into_iter().collect())
        }
    }

    fn write(&self, kind: u8, key: Vec<u8>, value: Vec<u8>) -> anyhow::Result<()> {
        self.memory
            .write()
            .entry(kind)
            .and_modify(|v| {
                v.insert(key.clone(), value.clone());
            })
            .or_insert(HashMap::from([(key, value)]));

        Ok(())
    }

    fn delete(&self, kind: u8, key: Option<Vec<u8>>) -> anyhow::Result<()> {
        if let Some(key) = key {
            self.memory
                .write()
                .get_mut(&kind)
                .context("couldn't find map")?
                .remove(&key);
        } else {
            self.memory.write().remove(&kind);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::sled::SledStorageProvider;
    use crate::DLCStoreProvider;

    macro_rules! sled_test {
        ($name: ident, $body: expr) => {
            #[test]
            fn $name() {
                let path = format!("{}{}", "test_files/sleddb/", std::stringify!($name));
                {
                    let storage = SledStorageProvider::new(&path);
                    #[allow(clippy::redundant_closure_call)]
                    $body(storage);
                }
                std::fs::remove_dir_all(path).unwrap();
            }
        };
    }

    sled_test!(write_key_and_value, |storage: SledStorageProvider| {
        let result = storage.write(
            1,
            "key".to_string().into_bytes(),
            "test".to_string().into_bytes(),
        );
        assert!(result.is_ok())
    });

    sled_test!(read_without_key, |storage: SledStorageProvider| {
        storage
            .write(
                1,
                "key".to_string().into_bytes(),
                "test".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                1,
                "key2".to_string().into_bytes(),
                "test2".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                2,
                "key3".to_string().into_bytes(),
                "test3".to_string().into_bytes(),
            )
            .unwrap();
        let result = storage.read(1, None).unwrap();

        assert_eq!(2, result.len());
    });

    sled_test!(read_with_key, |storage: SledStorageProvider| {
        storage
            .write(
                1,
                "key".to_string().into_bytes(),
                "test".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                1,
                "key2".to_string().into_bytes(),
                "test2".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                2,
                "key3".to_string().into_bytes(),
                "test3".to_string().into_bytes(),
            )
            .unwrap();
        let result = storage
            .read(1, Some("key2".to_string().into_bytes()))
            .unwrap();

        assert_eq!(1, result.len());
    });

    sled_test!(
        read_with_non_existing_key,
        |storage: SledStorageProvider| {
            let result = storage
                .read(1, Some("non_existing".to_string().into_bytes()))
                .unwrap();
            assert_eq!(0, result.len())
        }
    );

    sled_test!(delete_without_key, |storage: SledStorageProvider| {
        storage
            .write(
                1,
                "key".to_string().into_bytes(),
                "test".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                1,
                "key2".to_string().into_bytes(),
                "test2".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                2,
                "key3".to_string().into_bytes(),
                "test3".to_string().into_bytes(),
            )
            .unwrap();

        let result = storage.read(1, None).unwrap();
        assert_eq!(2, result.len());

        let result = storage.delete(1, None);
        assert!(result.is_ok());

        let result = storage.read(1, None).unwrap();
        assert_eq!(0, result.len());
    });

    sled_test!(delete_with_key, |storage: SledStorageProvider| {
        storage
            .write(
                1,
                "key".to_string().into_bytes(),
                "test".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                1,
                "key2".to_string().into_bytes(),
                "test2".to_string().into_bytes(),
            )
            .unwrap();
        storage
            .write(
                2,
                "key3".to_string().into_bytes(),
                "test3".to_string().into_bytes(),
            )
            .unwrap();

        let result = storage.read(1, None).unwrap();
        assert_eq!(2, result.len());

        let result = storage.delete(1, Some("key2".to_string().into_bytes()));
        assert!(result.is_ok());

        let result = storage.read(1, None).unwrap();
        assert_eq!(1, result.len());
    });
}
