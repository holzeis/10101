use crate::db;
use crate::event;
use crate::event::BackgroundTask;
use crate::event::EventInternal;
use crate::event::TaskStatus;
use crate::storage::TenTenOneNodeStorage;
use crate::trade::order;
use crate::trade::order::FailureReason;
use crate::trade::order::InvalidSubchannelOffer;
use crate::trade::position;
use crate::trade::position::handler::update_position_after_dlc_channel_creation_or_update;
use crate::trade::position::handler::update_position_after_dlc_closure;
use crate::trade::position::PositionState;
use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use bdk::bitcoin::secp256k1::PublicKey;
use bdk::TransactionDetails;
use bitcoin::hashes::hex::ToHex;
use bitcoin::Txid;
use dlc_messages::ChannelMessage;
use dlc_messages::Message;
use lightning::chain::transaction::OutPoint;
use lightning::ln::ChannelId;
use lightning::ln::PaymentHash;
use lightning::ln::PaymentPreimage;
use lightning::ln::PaymentSecret;
use lightning::sign::DelayedPaymentOutputDescriptor;
use lightning::sign::SpendableOutputDescriptor;
use lightning::sign::StaticPaymentOutputDescriptor;
use ln_dlc_node::channel::Channel;
use ln_dlc_node::dlc_message::DlcMessage;
use ln_dlc_node::dlc_message::SerializedDlcMessage;
use ln_dlc_node::node;
use ln_dlc_node::node::dlc_message_name;
use ln_dlc_node::node::event::NodeEvent;
use ln_dlc_node::node::rust_dlc_manager::DlcChannelId;
use ln_dlc_node::node::NodeInfo;
use ln_dlc_node::node::PaymentDetails;
use ln_dlc_node::node::RunningNode;
use ln_dlc_node::transaction::Transaction;
use ln_dlc_node::HTLCStatus;
use ln_dlc_node::MillisatAmount;
use ln_dlc_node::PaymentFlow;
use ln_dlc_node::PaymentInfo;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tracing::instrument;

#[derive(Clone)]
pub struct Node {
    pub inner: Arc<node::Node<TenTenOneNodeStorage, NodeStorage>>,
    _running: Arc<RunningNode>,
    // TODO: we should make this persistent as invoices might get paid later - but for now this is
    // good enough
    pub pending_usdp_invoices: Arc<parking_lot::Mutex<HashSet<bitcoin::hashes::sha256::Hash>>>,
}

impl Node {
    pub fn new(
        node: Arc<node::Node<TenTenOneNodeStorage, NodeStorage>>,
        running: RunningNode,
    ) -> Self {
        Self {
            inner: node,
            _running: Arc::new(running),
            pending_usdp_invoices: Arc::new(Default::default()),
        }
    }
}

pub struct Balances {
    pub on_chain: u64,
    pub off_chain: u64,
}

impl From<Balances> for crate::api::Balances {
    fn from(value: Balances) -> Self {
        Self {
            on_chain: value.on_chain,
            off_chain: value.off_chain,
        }
    }
}

pub struct WalletHistories {
    pub on_chain: Vec<TransactionDetails>,
    pub off_chain: Vec<PaymentDetails>,
}

impl Node {
    pub fn get_blockchain_height(&self) -> Result<u64> {
        self.inner.get_blockchain_height()
    }

    pub fn get_wallet_balances(&self) -> Result<Balances> {
        let on_chain = self.inner.get_on_chain_balance()?.confirmed;

        let off_chain = self.inner.get_dlc_channels_usable_balance()?;

        Ok(Balances {
            on_chain,
            off_chain: off_chain.to_sat(),
        })
    }

    pub fn get_wallet_histories(&self) -> Result<WalletHistories> {
        let on_chain = self.inner.get_on_chain_history()?;
        let off_chain = self.inner.get_off_chain_history()?;

        Ok(WalletHistories {
            on_chain,
            off_chain,
        })
    }

    pub fn process_incoming_dlc_messages(&self) {
        if !self
            .inner
            .dlc_message_handler
            .has_pending_messages_to_process()
        {
            return;
        }

        let messages = self
            .inner
            .dlc_message_handler
            .get_and_clear_received_messages();

        for (node_id, msg) in messages {
            let msg_name = dlc_message_name(&msg);
            if let Err(e) = self.process_dlc_message(node_id, msg) {
                tracing::error!(
                    from = %node_id,
                    kind = %msg_name,
                    "Failed to process incoming DLC message: {e:#}"
                );
            }
        }
    }

    /// [`process_dlc_message`] processes incoming dlc channel messages and updates the 10101
    /// position accordingly.
    /// - Any other message will be ignored.
    /// - Any dlc channel message that has already been processed will be skipped.
    ///
    /// If an offer is received [`ChannelMessage::Offer`], [`ChannelMessage::SettleOffer`],
    /// [`ChannelMessage::CollaborativeCloseOffer`], [`ChannelMessage::RenewOffer`] will get
    /// automatically accepted. Unless the maturity date of the offer is already outdated.
    ///
    /// FIXME(holzeis): This function manipulates different data objects in different data sources
    /// and should use a transaction to make all changes atomic. Not doing so risks of ending up in
    /// an inconsistent state. One way of fixing that could be to
    /// (1) use a single data source for the 10101 data and the rust-dlc data.
    /// (2) wrap the function into a db transaction which can be atomically rolled back on error or
    /// committed on success.
    fn process_dlc_message(&self, node_id: PublicKey, msg: Message) -> Result<()> {
        tracing::info!(
            from = %node_id,
            kind = %dlc_message_name(&msg),
            "Processing message"
        );

        let resp = match &msg {
            Message::OnChain(_) | Message::SubChannel(_) => {
                tracing::warn!("Ignoring unexpected dlc message.");
                None
            }
            Message::Channel(channel_msg) => {
                let inbound_msg = {
                    let mut conn = db::connection()?;
                    let serialized_inbound_message = SerializedDlcMessage::try_from(&msg)?;
                    let inbound_msg = DlcMessage::new(node_id, serialized_inbound_message, true)?;
                    match db::dlc_messages::DlcMessage::get(&mut conn, &inbound_msg.message_hash)? {
                        Some(_) => {
                            tracing::debug!(%node_id, kind=%dlc_message_name(&msg), "Received message that has already been processed, skipping.");
                            return Ok(());
                        }
                        None => inbound_msg,
                    }
                };

                let resp = self
                    .inner
                    .dlc_manager
                    .on_dlc_message(&msg, node_id)
                    .with_context(|| {
                        format!(
                            "Failed to handle {} message from {node_id}",
                            dlc_message_name(&msg)
                        )
                    })?;

                {
                    let mut conn = db::connection()?;
                    db::dlc_messages::DlcMessage::insert(&mut conn, inbound_msg)?;
                }

                match channel_msg {
                    ChannelMessage::Offer(offer) => {
                        let action = decide_subchannel_offer_action(
                            OffsetDateTime::from_unix_timestamp(
                                offer.contract_info.get_closest_maturity_date() as i64,
                            )
                            .expect("A contract should always have a valid maturity timestamp."),
                        );
                        self.process_dlc_channel_offer(offer.temporary_channel_id, action)?;
                    }
                    ChannelMessage::SettleOffer(offer) => {
                        self.inner
                            .accept_dlc_channel_collaborative_settlement(&offer.channel_id)
                            .with_context(|| {
                                format!(
                                    "Failed to accept DLC channel close offer for channel {}",
                                    hex::encode(offer.channel_id)
                                )
                            })?;
                    }
                    ChannelMessage::RenewOffer(r) => {
                        let channel_id_hex = r.channel_id.to_hex();

                        tracing::info!(
                            channel_id = %channel_id_hex,
                            "Automatically accepting renew offer"
                        );

                        // TODO: This is generally not safe. The coordinator will even send
                        // `RenewOffer`s in order to roll over, and these will not even be triggered
                        // by a user action.
                        let (accept_renew_offer, counterparty_pubkey) =
                            self.inner.dlc_manager.accept_renew_offer(&r.channel_id)?;

                        self.send_dlc_message(
                            counterparty_pubkey,
                            Message::Channel(ChannelMessage::RenewAccept(accept_renew_offer)),
                        )?;

                        let expiry_timestamp = OffsetDateTime::from_unix_timestamp(
                            r.contract_info.get_closest_maturity_date() as i64,
                        )?;
                        position::handler::handle_channel_renewal_offer(expiry_timestamp)?;
                    }
                    ChannelMessage::RenewRevoke(r) => {
                        let channel_id_hex = r.channel_id.to_hex();

                        tracing::info!(
                            channel_id = %channel_id_hex,
                            "Finished renew protocol"
                        );

                        let expiry_timestamp = self
                            .inner
                            .get_expiry_for_confirmed_dlc_channel(r.channel_id)?;

                        match db::get_order_in_filling()? {
                            Some(_) => {
                                let filled_order = order::handler::order_filled()
                                    .context("Cannot mark order as filled for confirmed DLC")?;

                                update_position_after_dlc_channel_creation_or_update(
                                    filled_order,
                                    expiry_timestamp,
                                )
                                .context("Failed to update position after DLC creation")?;
                            }
                            // If there is no order in `Filling` we must be rolling over.
                            None => {
                                tracing::info!(
                                    channel_id = %channel_id_hex,
                                    "Finished rolling over position"
                                );

                                position::handler::set_position_state(PositionState::Open)?;

                                event::publish(&EventInternal::BackgroundNotification(
                                    BackgroundTask::Rollover(TaskStatus::Success),
                                ));
                            }
                        };
                    }
                    ChannelMessage::Sign(signed) => {
                        let expiry_timestamp = self
                            .inner
                            .get_expiry_for_confirmed_dlc_channel(signed.channel_id)?;

                        let filled_order = order::handler::order_filled()
                            .context("Cannot mark order as filled for confirmed DLC")?;

                        update_position_after_dlc_channel_creation_or_update(
                            filled_order,
                            expiry_timestamp,
                        )
                        .context("Failed to update position after DLC creation")?;

                        // Sending always a recover dlc background notification success message here
                        // as we do not know if we might have reached this state after a restart.
                        // This event is only received by the UI at the moment indicating that the
                        // dialog can be closed. If the dialog is not open, this event would be
                        // simply ignored by the UI.
                        //
                        // FIXME(holzeis): We should not require that event and align the UI
                        // handling with waiting for an order execution in the happy case with
                        // waiting for an order execution after an in between restart. For now it
                        // was the easiest to go parallel to that implementation so that we don't
                        // have to touch it.
                        event::publish(&EventInternal::BackgroundNotification(
                            BackgroundTask::RecoverDlc(TaskStatus::Success),
                        ));
                    }
                    ChannelMessage::SettleConfirm(_) => {
                        tracing::debug!("Position based on DLC channel is being closed");

                        let filled_order = order::handler::order_filled()?;

                        update_position_after_dlc_closure(Some(filled_order))
                            .context("Failed to update position after DLC closure")?;

                        // In case of a restart.
                        event::publish(&EventInternal::BackgroundNotification(
                            BackgroundTask::RecoverDlc(TaskStatus::Success),
                        ));
                    }
                    ChannelMessage::CollaborativeCloseOffer(close_offer) => {
                        let channel_id_hex_string = close_offer.channel_id.to_hex();
                        tracing::info!(
                            channel_id = channel_id_hex_string,
                            node_id = node_id.to_string(),
                            "Received an offer to collaboratively close a channel"
                        );

                        // TODO(bonomat): we should verify that the proposed amount is acceptable
                        self.inner
                            .accept_dlc_channel_collaborative_close(&close_offer.channel_id)?;
                    }
                    _ => (),
                }

                resp
            }
        };

        if let Some(msg) = resp {
            self.send_dlc_message(node_id, msg.clone())?;
        }

        Ok(())
    }

    #[instrument(fields(channel_id = channel_id.to_hex()),skip_all, err(Debug))]
    fn process_dlc_channel_offer(
        &self,
        channel_id: DlcChannelId,
        action: SubchannelOfferAction,
    ) -> Result<()> {
        OffsetDateTime::now_utc();

        match &action {
            SubchannelOfferAction::Accept => {
                if let Err(error) = self
                    .inner
                    .accept_dlc_channel_offer(&channel_id)
                    .with_context(|| {
                        format!(
                            "Failed to accept DLC channel offer for channel {}",
                            hex::encode(channel_id)
                        )
                    })
                {
                    tracing::error!(
                        "Could not accept dlc channel offer {error:#}. Continuing with rejecting it"
                    );
                    self.process_dlc_channel_offer(channel_id, SubchannelOfferAction::RejectError)?
                }
            }
            SubchannelOfferAction::RejectOutdated | SubchannelOfferAction::RejectError => {
                let (invalid_offer_reason, invalid_offer_msg) =
                    if let SubchannelOfferAction::RejectOutdated = action {
                        let invalid_offer_msg = "Offer outdated, rejecting subchannel offer";
                        tracing::warn!(invalid_offer_msg);
                        (InvalidSubchannelOffer::Outdated, invalid_offer_msg)
                    } else {
                        let invalid_offer_msg = "Offer is unacceptable, rejecting subchannel offer";
                        tracing::warn!(invalid_offer_msg);
                        (InvalidSubchannelOffer::Unacceptable, invalid_offer_msg)
                    };
                self.inner
                    .reject_dlc_channel_offer(&channel_id)
                    .with_context(|| {
                        format!(
                            "Failed to reject DLC channel offer for channel {}",
                            hex::encode(channel_id)
                        )
                    })?;

                order::handler::order_failed(
                    None,
                    FailureReason::InvalidDlcOffer(invalid_offer_reason),
                    anyhow!(invalid_offer_msg),
                )
                .context("Could not set order to failed")?;
            }
        }

        Ok(())
    }

    #[instrument(fields(channel_id = channel_id.to_hex()),skip_all, err(Debug))]
    pub fn process_subchannel_offer(
        &self,
        channel_id: ChannelId,
        action: SubchannelOfferAction,
    ) -> Result<()> {
        OffsetDateTime::now_utc();

        match &action {
            SubchannelOfferAction::Accept => {
                if let Err(error) = self
                    .inner
                    .accept_sub_channel_offer(&channel_id)
                    .with_context(|| {
                        format!(
                            "Failed to accept DLC channel offer for channel {}",
                            hex::encode(channel_id.0)
                        )
                    })
                {
                    tracing::error!(
                        "Could not accept subchannel offer {error:#}. Continuing with rejecting it"
                    );
                    self.process_subchannel_offer(channel_id, SubchannelOfferAction::RejectError)?
                }
            }
            SubchannelOfferAction::RejectOutdated | SubchannelOfferAction::RejectError => {
                let (invalid_offer_reason, invalid_offer_msg) =
                    if let SubchannelOfferAction::RejectOutdated = action {
                        let invalid_offer_msg = "Offer outdated, rejecting subchannel offer";
                        tracing::warn!(invalid_offer_msg);
                        (InvalidSubchannelOffer::Outdated, invalid_offer_msg)
                    } else {
                        let invalid_offer_msg = "Offer is unacceptable, rejecting subchannel offer";
                        tracing::warn!(invalid_offer_msg);
                        (InvalidSubchannelOffer::Unacceptable, invalid_offer_msg)
                    };
                self.inner
                    .reject_sub_channel_offer(&channel_id)
                    .with_context(|| {
                        format!(
                            "Failed to reject DLC channel offer for channel {}",
                            hex::encode(channel_id.0)
                        )
                    })?;

                order::handler::order_failed(
                    None,
                    FailureReason::InvalidDlcOffer(invalid_offer_reason),
                    anyhow!(invalid_offer_msg),
                )
                .context("Could not set order to failed")?;
            }
        }

        Ok(())
    }

    pub fn send_dlc_message(&self, node_id: PublicKey, msg: Message) -> Result<()> {
        tracing::info!(
            to = %node_id,
            kind = %dlc_message_name(&msg),
            "Sending message"
        );

        self.inner
            .event_handler
            .publish(NodeEvent::SendDlcMessage {
                peer: node_id,
                msg: msg.clone(),
            })?;

        Ok(())
    }

    pub async fn keep_connected(&self, peer: NodeInfo) {
        let reconnect_interval = Duration::from_secs(1);
        loop {
            let connection_closed_future = match self.inner.connect(peer).await {
                Ok(fut) => fut,
                Err(e) => {
                    tracing::warn!(
                        %peer,
                        ?reconnect_interval,
                        "Connection failed: {e:#}; reconnecting"
                    );

                    tokio::time::sleep(reconnect_interval).await;
                    continue;
                }
            };

            connection_closed_future.await;
            tracing::debug!(
                %peer,
                ?reconnect_interval,
                "Connection lost; reconnecting"
            );

            tokio::time::sleep(reconnect_interval).await;
        }
    }
}

pub(crate) fn decide_subchannel_offer_action(
    maturity_timestamp: OffsetDateTime,
) -> SubchannelOfferAction {
    let mut action = SubchannelOfferAction::Accept;
    if OffsetDateTime::now_utc().gt(&maturity_timestamp) {
        action = SubchannelOfferAction::RejectOutdated;
    }
    action
}

pub enum SubchannelOfferAction {
    Accept,
    /// The offer was outdated, hence we need to reject the offer
    RejectOutdated,
    // /// The offer was invalid, hence we need to reject the offer
    // RejectInvalid,
    /// We had an error accepting, hence, we try to reject instead
    RejectError,
}

#[derive(Clone)]
pub struct NodeStorage;

impl node::Storage for NodeStorage {
    // Payments

    fn insert_payment(&self, payment_hash: PaymentHash, info: PaymentInfo) -> Result<()> {
        db::insert_payment(payment_hash, info)
    }
    fn merge_payment(
        &self,
        payment_hash: &PaymentHash,
        flow: PaymentFlow,
        amt_msat: MillisatAmount,
        fee_msat: MillisatAmount,
        htlc_status: HTLCStatus,
        preimage: Option<PaymentPreimage>,
        secret: Option<PaymentSecret>,
        funding_txid: Option<Txid>,
    ) -> Result<()> {
        match db::get_payment(*payment_hash)? {
            Some(_) => {
                db::update_payment(
                    *payment_hash,
                    htlc_status,
                    amt_msat,
                    fee_msat,
                    preimage,
                    secret,
                    funding_txid,
                )?;
            }
            None => {
                db::insert_payment(
                    *payment_hash,
                    PaymentInfo {
                        preimage,
                        secret,
                        status: htlc_status,
                        amt_msat,
                        fee_msat,
                        flow,
                        timestamp: OffsetDateTime::now_utc(),
                        description: "".to_string(),
                        invoice: None,
                        funding_txid,
                    },
                )?;
            }
        }

        Ok(())
    }
    fn get_payment(
        &self,
        payment_hash: &PaymentHash,
    ) -> Result<Option<(PaymentHash, PaymentInfo)>> {
        db::get_payment(*payment_hash)
    }
    fn all_payments(&self) -> Result<Vec<(PaymentHash, PaymentInfo)>> {
        db::get_payments()
    }

    // Spendable outputs

    fn insert_spendable_output(&self, descriptor: SpendableOutputDescriptor) -> Result<()> {
        use SpendableOutputDescriptor::*;
        let outpoint = match &descriptor {
            // Static outputs don't need to be persisted because they pay directly to an address
            // owned by the on-chain wallet
            StaticOutput { .. } => return Ok(()),
            DelayedPaymentOutput(DelayedPaymentOutputDescriptor { outpoint, .. }) => outpoint,
            StaticPaymentOutput(StaticPaymentOutputDescriptor { outpoint, .. }) => outpoint,
        };

        db::insert_spendable_output(*outpoint, descriptor)
    }

    fn get_spendable_output(
        &self,
        outpoint: &OutPoint,
    ) -> Result<Option<SpendableOutputDescriptor>> {
        db::get_spendable_output(*outpoint)
    }

    fn delete_spendable_output(&self, outpoint: &OutPoint) -> Result<()> {
        db::delete_spendable_output(*outpoint)
    }

    fn all_spendable_outputs(&self) -> Result<Vec<SpendableOutputDescriptor>> {
        db::get_spendable_outputs()
    }

    // Channels

    fn upsert_channel(&self, channel: Channel) -> Result<()> {
        db::upsert_channel(channel)
    }

    fn get_channel(&self, user_channel_id: &str) -> Result<Option<Channel>> {
        db::get_channel(user_channel_id)
    }

    fn all_non_pending_channels(&self) -> Result<Vec<Channel>> {
        db::get_all_non_pending_channels()
    }

    fn get_announced_channel(&self, counterparty_pubkey: PublicKey) -> Result<Option<Channel>> {
        db::get_announced_channel(counterparty_pubkey)
    }

    fn get_channel_by_payment_hash(&self, payment_hash: String) -> Result<Option<Channel>> {
        db::get_channel_by_payment_hash(payment_hash)
    }

    // Transactions

    fn upsert_transaction(&self, transaction: Transaction) -> Result<()> {
        db::upsert_transaction(transaction)
    }

    fn get_transaction(&self, txid: &str) -> Result<Option<Transaction>> {
        db::get_transaction(txid)
    }

    fn all_transactions_without_fees(&self) -> Result<Vec<Transaction>> {
        db::get_all_transactions_without_fees()
    }
}
