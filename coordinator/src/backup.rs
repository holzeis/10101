use anyhow::Result;
use axum::body::Bytes;
use coordinator_commons::Restore;
use futures::Stream;
use futures::TryStreamExt;
use sled::Db;
use std::io;
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;

const BACKUPS_DIRECTORY: &str = "user_backups";

pub struct SledBackup {
    db: Db,
}

impl SledBackup {
    pub fn new(data_dir: String) -> Self {
        SledBackup {
            db: sled::open(format!("{data_dir}/{BACKUPS_DIRECTORY}")).expect("valid path"),
        }
    }

    pub fn restore(&self, node_id: String) -> Result<Vec<Restore>> {
        let tree = self.db.open_tree(node_id)?;

        let mut backup = vec![];
        for entry in tree.into_iter() {
            let entry = entry?;
            let key = String::from_utf8(entry.0.to_vec())?;
            let value = entry.1.to_vec();

            let keys = key
                .split('/')
                .map(|key| key.to_string())
                .collect::<Vec<String>>();

            let (kind, key) = keys.split_first().expect("keys to be long enough");
            backup.push(Restore {
                kind: kind.as_str().try_into()?,
                key: key.join("/"),
                value,
            });
        }

        Ok(backup)
    }

    pub async fn backup<S: Stream<Item = Result<Bytes, axum::Error>>>(
        &self,
        node_id: String,
        key: String,
        stream: S,
    ) -> Result<()> {
        // Convert the stream into an `AsyncRead`.
        let stream_with_io_error = stream.map_err(|err| io::Error::new(io::ErrorKind::Other, err));
        let stream_reader = StreamReader::new(stream_with_io_error);
        futures::pin_mut!(stream_reader);

        let mut value = vec![];
        stream_reader.read_to_end(&mut value).await?;

        tracing::debug!(%node_id, key, "Create user backup");

        let tree = self.db.open_tree(node_id)?;
        tree.insert(key, value)?;
        tree.flush()?;
        Ok(())
    }

    pub fn delete(&self, node_id: String, key: String) -> Result<()> {
        let tree = self.db.open_tree(node_id)?;
        tree.remove(key)?;
        tree.flush()?;
        Ok(())
    }
}
