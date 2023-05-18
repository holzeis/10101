use anyhow::bail;
use anyhow::Context;
use anyhow::Error;
use anyhow::Result;
use bdk::blockchain::Blockchain;
use bdk::blockchain::EsploraBlockchain;
use bdk::blockchain::GetHeight;
use bdk::database::BatchDatabase;
use bdk::wallet::AddressIndex;
use bdk::FeeRate;
use bdk::SignOptions;
use bdk::SyncOptions;
use bdk::TransactionDetails;
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::BlockHash;
use bitcoin::Network;
use bitcoin::Script;
use bitcoin::Transaction;
use bitcoin::Txid;
use lightning::chain::chaininterface::BroadcasterInterface;
use lightning::chain::chaininterface::ConfirmationTarget;
use lightning::chain::chaininterface::FeeEstimator;
use lightning::chain::chaininterface::FEERATE_FLOOR_SATS_PER_KW;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

pub struct Wallet<D>
where
    D: BatchDatabase,
{
    // A BDK blockchain used for wallet sync.
    pub(crate) blockchain: Arc<EsploraBlockchain>,
    // A BDK on-chain wallet.
    inner: Mutex<bdk::Wallet<D>>,
    // A cache storing the most recently retrieved fee rate estimations.
    fee_rate_cache: RwLock<HashMap<ConfirmationTarget, FeeRate>>,
    settings: RwLock<WalletSettings>,
    runtime_handle: tokio::runtime::Handle,
}

#[derive(Clone)]
pub struct WalletSettings {
    pub fallback_tx_fee_rate_normal: u32,
    pub fallback_tx_fee_rate_high_priority: u32,
    pub max_allowed_tx_fee_rate_when_opening_channel: Option<u32>,
}

impl Default for WalletSettings {
    fn default() -> Self {
        Self {
            fallback_tx_fee_rate_normal: 2000,
            fallback_tx_fee_rate_high_priority: 5000,
            max_allowed_tx_fee_rate_when_opening_channel: None,
        }
    }
}

impl<D> Wallet<D>
where
    D: BatchDatabase,
{
    pub(crate) fn new(
        blockchain: EsploraBlockchain,
        wallet: bdk::Wallet<D>,
        runtime_handle: tokio::runtime::Handle,
    ) -> Self {
        let inner = Mutex::new(wallet);
        let fee_rate_cache = RwLock::new(HashMap::new());
        let settings = RwLock::new(WalletSettings::default());

        Self {
            blockchain: Arc::new(blockchain),
            inner,
            fee_rate_cache,
            runtime_handle,
            settings,
        }
    }

    pub async fn update_settings(&self, settings: WalletSettings) {
        *self.settings.write().await = settings;
    }

    pub async fn settings(&self) -> WalletSettings {
        self.settings.read().await.clone()
    }

    /// Update fee estimates and the internal BDK wallet database with
    /// the blockchain.
    pub async fn sync(&self) -> Result<()> {
        self.update_fee_estimates()
            .await
            .context("Failed to update fee estimates")?;

        let wallet_lock = self.inner.lock().await;
        match wallet_lock
            .sync(&self.blockchain, SyncOptions { progress: None })
            .await
        {
            Err(bdk::Error::Esplora(e)) => match *e {
                bdk::blockchain::esplora::EsploraError::Reqwest(e) => {
                    tracing::error!(
                        "Sync failed due to HTTP connection error, retrying once: {}",
                        e
                    );

                    tokio::time::sleep(Duration::from_secs(1)).await;
                    wallet_lock
                        .sync(&self.blockchain, SyncOptions { progress: None })
                        .await
                        .context("Sync failed due to HTTP connection error after retry")?
                }
                _ => bail!(e),
            },
            Err(e) => {
                bail!(e);
            }
            Ok(()) => {}
        };

        Ok(())
    }

    pub(crate) async fn update_fee_estimates(&self) -> Result<()> {
        let mut locked_fee_rate_cache = self.fee_rate_cache.write().await;

        let confirmation_targets = vec![
            ConfirmationTarget::Background,
            ConfirmationTarget::Normal,
            ConfirmationTarget::HighPriority,
        ];
        for target in confirmation_targets {
            let num_blocks = match target {
                ConfirmationTarget::Background => 12,
                ConfirmationTarget::Normal => 6,
                ConfirmationTarget::HighPriority => 3,
            };

            let est_fee_rate = self.blockchain.estimate_fee(num_blocks).await;

            match est_fee_rate {
                Ok(rate) => {
                    locked_fee_rate_cache.insert(target, rate);
                    tracing::trace!(
                        "Fee rate estimation updated: {} sats/kwu",
                        rate.fee_wu(1000)
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to update fee rate estimation: {}", e);
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn create_funding_transaction(
        &self,
        output_script: Script,
        value_sats: u64,
        confirmation_target: ConfirmationTarget,
    ) -> Result<Transaction, Error> {
        let fee_rate = self.estimate_fee_rate(confirmation_target);

        let locked_wallet = self.inner.lock().await;
        let mut tx_builder = locked_wallet.build_tx();

        tx_builder
            .add_recipient(output_script, value_sats)
            .fee_rate(fee_rate)
            .enable_rbf();

        let mut psbt = match tx_builder.finish() {
            Ok((psbt, _)) => {
                tracing::trace!("Created funding PSBT: {:?}", psbt);
                psbt
            }
            Err(err) => {
                tracing::error!("Failed to create funding transaction: {}", err);
                return Err(err.into());
            }
        };

        match locked_wallet.sign(&mut psbt, SignOptions::default()) {
            Ok(finalized) => {
                if !finalized {
                    bail!("Onchain transaction failed");
                }
            }
            Err(err) => {
                tracing::error!("Failed to create funding transaction: {}", err);
                return Err(err.into());
            }
        }

        Ok(psbt.extract_tx())
    }

    pub(crate) fn get_new_address(&self) -> Result<bitcoin::Address, Error> {
        let address_info = tokio::task::block_in_place(move || {
            self.runtime_handle
                .block_on(async move { self.inner.lock().await.get_address(AddressIndex::New) })
        })?;

        Ok(address_info.address)
    }

    pub(crate) fn get_last_unused_address(&self) -> Result<bitcoin::Address, Error> {
        let address_info = tokio::task::block_in_place(move || {
            self.runtime_handle.block_on(async move {
                self.inner
                    .lock()
                    .await
                    .get_address(AddressIndex::LastUnused)
            })
        })?;

        Ok(address_info.address)
    }

    pub(crate) async fn get_balance(&self) -> Result<bdk::Balance, Error> {
        Ok(self.inner.lock().await.get_balance()?)
    }

    /// Send funds to the given address.
    ///
    /// If `amount_msat_or_drain` is `None` the wallet will be drained, i.e., all available funds
    /// will be spent.
    #[allow(dead_code)]
    pub(crate) async fn send_to_address(
        &self,
        address: &bitcoin::Address,
        amount_msat_or_drain: Option<u64>,
    ) -> Result<Txid> {
        let confirmation_target = ConfirmationTarget::Normal;
        let fee_rate = self.estimate_fee_rate(confirmation_target);

        let tx = {
            let locked_wallet = self.inner.lock().await;
            let mut tx_builder = locked_wallet.build_tx();

            if let Some(amount_sats) = amount_msat_or_drain {
                tx_builder
                    .add_recipient(address.script_pubkey(), amount_sats)
                    .fee_rate(fee_rate)
                    .enable_rbf();
            } else {
                tx_builder
                    .drain_wallet()
                    .drain_to(address.script_pubkey())
                    .fee_rate(fee_rate)
                    .enable_rbf();
            }

            let mut psbt = match tx_builder.finish() {
                Ok((psbt, _)) => {
                    tracing::trace!("Created PSBT: {:?}", psbt);
                    psbt
                }
                Err(err) => {
                    bail!(err)
                }
            };

            match locked_wallet.sign(&mut psbt, SignOptions::default()) {
                Ok(finalized) => {
                    if !finalized {
                        bail!("On chain creation failed");
                    }
                }
                Err(err) => {
                    bail!(err)
                }
            }
            psbt.extract_tx()
        };

        self.broadcast_transaction(&tx);

        let txid = tx.txid();

        if let Some(amount_sats) = amount_msat_or_drain {
            tracing::info!(
                "Created new transaction {} sending {}sats on-chain to address {}",
                txid,
                amount_sats,
                address
            );
        } else {
            tracing::info!(
                "Created new transaction {} sending all available on-chain funds to address {}",
                txid,
                address
            );
        }

        Ok(txid)
    }

    fn estimate_fee_rate(&self, confirmation_target: ConfirmationTarget) -> FeeRate {
        let (fee_rate_cache, settings) = tokio::task::block_in_place(move || {
            self.runtime_handle.block_on(async move {
                (
                    self.fee_rate_cache.read().await.clone(),
                    self.settings.read().await.clone(),
                )
            })
        });

        let fallback_sats_kwu = match confirmation_target {
            ConfirmationTarget::Background => FEERATE_FLOOR_SATS_PER_KW,
            ConfirmationTarget::Normal => settings.fallback_tx_fee_rate_normal,
            ConfirmationTarget::HighPriority => settings.fallback_tx_fee_rate_high_priority,
        };

        // We'll fall back on this, if we really don't have any other information.
        let fallback_rate = FeeRate::from_sat_per_kwu(fallback_sats_kwu as f32);

        *fee_rate_cache
            .get(&confirmation_target)
            .unwrap_or(&fallback_rate)
    }

    pub fn tip(&self) -> Result<(u32, BlockHash)> {
        let ret = tokio::task::block_in_place(move || {
            self.runtime_handle.block_on(async move {
                anyhow::Ok((
                    self.blockchain.get_height().await?,
                    self.blockchain.get_tip_hash().await?,
                ))
            })
        })?;

        Ok(ret)
    }

    pub async fn on_chain_transaction_list(&self) -> Result<Vec<TransactionDetails>> {
        let wallet_lock = self.inner.lock().await;
        wallet_lock
            .list_transactions(false)
            .context("Failed to list on chain transactions")
    }

    pub fn network(&self) -> Result<Network> {
        let network = tokio::task::block_in_place(move || {
            self.runtime_handle
                .block_on(async move { self.inner.lock().await.network() })
        });

        Ok(network)
    }
}

impl<D> FeeEstimator for Wallet<D>
where
    D: BatchDatabase,
{
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        (self.estimate_fee_rate(confirmation_target).fee_wu(1000) as u32)
            .max(FEERATE_FLOOR_SATS_PER_KW)
    }
}

impl<D> BroadcasterInterface for Wallet<D>
where
    D: BatchDatabase,
{
    fn broadcast_transaction(&self, tx: &Transaction) {
        tracing::info!(txid = %tx.txid(), raw_tx = %serialize_hex(&tx), "Broadcasting transaction");

        tokio::task::block_in_place(move || {
            self.runtime_handle.block_on(async move {
                if let Err(err) = self.blockchain.broadcast(tx).await {
                    tracing::error!("Failed to broadcast transaction: {err:#}");
                }
            })
        });
    }
}