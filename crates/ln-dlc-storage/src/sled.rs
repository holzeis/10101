use crate::DLCStoreProvider;
use anyhow::bail;
use anyhow::ensure;
use parking_lot::RwLock;
use sled::Db;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct SledStorageProvider {
    db: Db,
}

impl SledStorageProvider {
    pub fn new(path: &str) -> Self {
        SledStorageProvider {
            db: sled::open(path).expect("valid path"),
        }
    }
}

impl DLCStoreProvider for SledStorageProvider {
    fn read(&self, key: Vec<String>) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        ensure!(!key.is_empty(), "missing key");
        if key.len() == 1 {
            let path = key.first().expect("key is not long enough");
            let result = self.db.open_tree(path)?;
            let result = result
                .into_iter()
                .map(|x| {
                    (
                        String::from_utf8(x.clone().expect("to not fail").0.to_vec())
                            .expect("to fit into utf8"),
                        x.expect("to not fail").1.to_vec(),
                    )
                })
                .collect::<Vec<(String, Vec<u8>)>>();

            Ok(result)
        } else {
            let (key, path) = key.split_last().expect("key is not long enough");
            let result = self.db.open_tree(path.join("/"))?;

            let result = match result.get(key)? {
                Some(value) => vec![(key.clone(), value.to_vec())],
                None => vec![],
            };

            Ok(result)
        }
    }

    fn write(&self, key: Vec<String>, value: Vec<u8>) -> anyhow::Result<()> {
        ensure!(!key.is_empty(), "missing key");
        if key.len() == 1 {
            let path = key.first().expect("key is not long enough");
            self.db.open_tree(path)?.insert("", value)?;
        } else {
            let (key, path) = key.split_last().expect("key is not long enough");
            self.db.open_tree(path.join("/"))?.insert(key, value)?;
        }

        self.db.flush()?;
        Ok(())
    }

    fn delete(&self, key: Vec<String>) -> anyhow::Result<()> {
        ensure!(!key.is_empty(), "missing key");

        if key.len() == 1 {
            let path = key.first().expect("key is not long enough");
            self.db.open_tree(path)?.clear()?;
        } else {
            let (key, path) = key.split_last().expect("key is not long enough");
            self.db.open_tree(path.join("/"))?.remove(key)?;
        }

        self.db.flush()?;
        Ok(())
    }
}

type InMemoryStore = Arc<RwLock<HashMap<String, HashMap<String, Vec<u8>>>>>;

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

    fn get_path_and_key(&self, keys: Vec<String>) -> anyhow::Result<(String, String)> {
        match keys.len() {
            0 => bail!("missing key"),
            1 => Ok((
                keys.first().expect("key is not long enough").to_string(),
                "".to_string(),
            )),
            _ => {
                let (key, path) = keys.split_last().expect("key is not long enough");
                Ok((path.join("/"), key.to_string()))
            }
        }
    }
}

impl DLCStoreProvider for InMemoryDLCStoreProvider {
    fn read(&self, keys: Vec<String>) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let (path, key) = self.get_path_and_key(keys)?;

        if key.is_empty() {
            match self.memory.read().get(&path) {
                Some(store) => Ok(store.clone().into_iter().collect()),
                None => Ok(vec![]),
            }
        } else {
            let store = self.memory.read();
            let store = match store.get(&path) {
                Some(store) => store,
                None => return Ok(vec![]),
            };

            if let Some(value) = store.get(&key) {
                Ok(vec![(key.to_string(), value.clone())])
            } else {
                Ok(vec![])
            }
        }
    }

    fn write(&self, keys: Vec<String>, value: Vec<u8>) -> anyhow::Result<()> {
        let (path, key) = self.get_path_and_key(keys)?;

        self.memory
            .write()
            .entry(path)
            .and_modify(|v| {
                v.insert(key.clone(), value.clone());
            })
            .or_insert(HashMap::from([(key, value)]));

        Ok(())
    }

    fn delete(&self, keys: Vec<String>) -> anyhow::Result<()> {
        let (path, key) = self.get_path_and_key(keys)?;
        if key.is_empty() {
            self.memory.write().remove(&path);
        } else {
            self.memory
                .write()
                .get_mut(&path)
                .expect("path to exist")
                .remove(&key);
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

    sled_test!(write_without_keys, |storage: SledStorageProvider| {
        let result = storage.write(vec![], "test".to_string().into_bytes());
        assert!(result.is_err())
    });

    sled_test!(write_with_path_only, |storage: SledStorageProvider| {
        let result = storage.write(vec!["path".to_string()], "test".to_string().into_bytes());
        assert!(result.is_ok())
    });

    sled_test!(write_with_path_and_keys, |storage: SledStorageProvider| {
        let result = storage.write(
            vec!["path".to_string(), "key1".to_string(), "key2".to_string()],
            "test".to_string().into_bytes(),
        );
        assert!(result.is_ok())
    });

    sled_test!(read_without_keys, |storage: SledStorageProvider| {
        let result = storage.read(vec![]);
        assert!(result.is_err())
    });

    sled_test!(read_with_path_only, |storage: SledStorageProvider| {
        storage
            .write(vec!["path".to_string()], "value".to_string().into_bytes())
            .expect("to write");
        storage
            .write(
                vec!["path".to_string(), "key1".to_string()],
                "value1".to_string().into_bytes(),
            )
            .expect("to write");
        storage
            .write(
                vec!["path".to_string(), "key2".to_string()],
                "value2".to_string().into_bytes(),
            )
            .expect("to write");

        let result = storage.read(vec!["path".to_string()]).expect("to read");
        assert_eq!(3, result.len())
    });

    sled_test!(read_with_path_and_keys, |storage: SledStorageProvider| {
        storage
            .write(vec!["path".to_string()], "value".to_string().into_bytes())
            .expect("to write");
        storage
            .write(
                vec!["path".to_string(), "key1".to_string()],
                "value1".to_string().into_bytes(),
            )
            .expect("to write");
        storage
            .write(
                vec!["path".to_string(), "key2".to_string()],
                "value2".to_string().into_bytes(),
            )
            .expect("to write");

        let result = storage
            .read(vec!["path".to_string(), "key1".to_string()])
            .expect("to read");
        assert_eq!(1, result.len())
    });

    sled_test!(
        read_with_non_existing_path,
        |storage: SledStorageProvider| {
            storage
                .write(vec!["path".to_string()], "value".to_string().into_bytes())
                .expect("to write");
            storage
                .write(
                    vec!["path".to_string(), "key1".to_string()],
                    "value1".to_string().into_bytes(),
                )
                .expect("to write");
            storage
                .write(
                    vec!["path".to_string(), "key2".to_string()],
                    "value2".to_string().into_bytes(),
                )
                .expect("to write");

            let result = storage
                .read(vec!["non_existing".to_string()])
                .expect("to read");
            assert_eq!(0, result.len())
        }
    );

    sled_test!(
        read_with_non_existing_path_and_keys,
        |storage: SledStorageProvider| {
            storage
                .write(vec!["path".to_string()], "value".to_string().into_bytes())
                .expect("to write");
            storage
                .write(
                    vec!["path".to_string(), "key1".to_string()],
                    "value1".to_string().into_bytes(),
                )
                .expect("to write");
            storage
                .write(
                    vec!["path".to_string(), "key2".to_string()],
                    "value2".to_string().into_bytes(),
                )
                .expect("to write");

            let result = storage
                .read(vec!["non_existing".to_string(), "key1".to_string()])
                .expect("to read");
            assert_eq!(0, result.len())
        }
    );

    sled_test!(delete_without_keys, |storage: SledStorageProvider| {
        let result = storage.delete(vec![]);
        assert!(result.is_err())
    });

    sled_test!(delete_with_path_only, |storage: SledStorageProvider| {
        storage
            .write(vec!["path".to_string()], "value".to_string().into_bytes())
            .expect("to write");

        storage.delete(vec!["path".to_string()]).expect("to delete");

        let result = storage.read(vec!["path".to_string()]).expect("to read");
        assert_eq!(0, result.len())
    });

    sled_test!(delete_with_path_and_keys, |storage: SledStorageProvider| {
        storage
            .write(
                vec!["path".to_string(), "key1".to_string()],
                "value1".to_string().into_bytes(),
            )
            .expect("to write");
        storage
            .write(
                vec!["path".to_string(), "key2".to_string()],
                "value2".to_string().into_bytes(),
            )
            .expect("to write");

        storage
            .delete(vec!["path".to_string(), "key1".to_string()])
            .expect("to delete");

        let result = storage.read(vec!["path".to_string()]).expect("to read");
        assert_eq!(1, result.len())
    });

    sled_test!(
        delete_with_non_existing_path,
        |storage: SledStorageProvider| {
            storage
                .write(
                    vec!["path".to_string(), "key1".to_string()],
                    "value1".to_string().into_bytes(),
                )
                .expect("to write");
            storage
                .write(
                    vec!["path".to_string(), "key2".to_string()],
                    "value2".to_string().into_bytes(),
                )
                .expect("to write");

            storage
                .delete(vec!["non_existing".to_string()])
                .expect("to delete");

            let result = storage.read(vec!["path".to_string()]).expect("to read");
            assert_eq!(2, result.len())
        }
    );

    sled_test!(
        delete_with_non_existing_path_and_keys,
        |storage: SledStorageProvider| {
            storage
                .write(
                    vec!["path".to_string(), "key1".to_string()],
                    "value1".to_string().into_bytes(),
                )
                .expect("to write");
            storage
                .write(
                    vec!["path".to_string(), "key2".to_string()],
                    "value2".to_string().into_bytes(),
                )
                .expect("to write");

            storage
                .delete(vec!["non_existing".to_string(), "key1".to_string()])
                .expect("to delete");

            let result = storage.read(vec!["path".to_string()]).expect("to read");
            assert_eq!(2, result.len())
        }
    );
}
