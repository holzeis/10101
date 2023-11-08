use anyhow::Result;
use bitcoin::secp256k1::ecdsa::Signature;
use bitcoin::secp256k1::PublicKey;
use coordinator_commons::Backup;
use coordinator_commons::DeleteBackup;
use coordinator_commons::Restore;
use sled::Db;

const BACKUPS_DIRECTORY: &str = "user_backups";

/// Holds the user backups in a sled database
///
/// TODO(holzeis): This is fine for now, once we grow we should consider moving that into a dedicate
/// KV database, potentially to a managed service.
pub struct SledBackup {
    db: Db,
}

impl SledBackup {
    pub fn new(data_dir: String) -> Self {
        SledBackup {
            db: sled::open(format!("{data_dir}/{BACKUPS_DIRECTORY}")).expect("valid path"),
        }
    }

    pub fn restore(&self, node_id: PublicKey, signature: Signature) -> Result<Vec<Restore>> {
        let message = node_id.to_string().as_bytes().to_vec();
        let message = orderbook_commons::create_sign_message(message);
        signature.verify(&message, &node_id)?;

        tracing::debug!(%node_id, "Restoring backup");
        let tree = self.db.open_tree(node_id.to_string())?;

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

    pub async fn backup(&self, node_id: PublicKey, backup: Backup) -> Result<()> {
        backup.verify(&node_id)?;

        tracing::debug!(%node_id, backup.key, "Create user backup");
        let tree = self.db.open_tree(node_id.to_string())?;
        tree.insert(backup.key, backup.value)?;
        tree.flush()?;
        Ok(())
    }

    pub fn delete(&self, node_id: PublicKey, backup: DeleteBackup) -> Result<()> {
        backup.verify(&node_id)?;

        tracing::debug!(%node_id, key=backup.key, "Deleting user backup");

        let tree = self.db.open_tree(node_id.to_string())?;
        tree.remove(backup.key)?;
        tree.flush()?;
        Ok(())
    }
}
