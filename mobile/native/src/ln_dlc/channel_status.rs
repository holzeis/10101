use crate::event;
use crate::ln_dlc::node::Node;
use anyhow::Result;
use ln_dlc_node::node::rust_dlc_manager::channel::signed_channel::SignedChannel;
use ln_dlc_node::node::rust_dlc_manager::channel::signed_channel::SignedChannelState;
use ln_dlc_node::node::rust_dlc_manager::subchannel::SubChannel;
use std::borrow::Borrow;
use std::time::Duration;

const UPDATE_CHANNEL_STATUS_INTERVAL: Duration = Duration::from_secs(5);

/// The status of the app channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    /// No channel is open.
    ///
    /// This means that it is possible to open a new DLC channel. This does _not_ indicate if
    /// there was a previous channel nor does it imply that a previous channel was completely
    /// closed i.e. there might be pending transactions.
    NotOpen,
    /// The DLC channel is open but without an active DLC DLC attached to it.
    Open,
    /// The DLC channel has an open DLC attached to it.
    WithPosition,
    /// The DLC in the channel is currently being settled into the channel
    Settling,
    /// The DLC in the channel is currently being renewed
    Renewing,
    /// The channel is being closed
    Closing,
    /// The status of the channel is not known.
    Unknown,
}

pub async fn track_channel_status(node: impl Borrow<Node>) {
    let mut cached_status = ChannelStatus::Unknown;
    loop {
        tracing::trace!("Tracking channel status");

        let status = channel_status(node.borrow())
            .await
            .map_err(|e| {
                tracing::error!("Could not compute LN-DLC channel status: {e:#}");
            })
            .unwrap_or(ChannelStatus::Unknown);

        if status != cached_status {
            tracing::info!(?status, "Channel status update");
            event::publish(&event::EventInternal::ChannelStatusUpdate(status));
            cached_status = status;
        }

        tokio::time::sleep(UPDATE_CHANNEL_STATUS_INTERVAL).await;
    }
}

/// Figure out the status of the current channel.
async fn channel_status(node: impl Borrow<Node>) -> Result<ChannelStatus> {
    let node: &Node = node.borrow();
    let node = &node.inner;

    let dlc_channels = node.list_signed_dlc_channels()?;
    if dlc_channels.len() > 1 {
        tracing::warn!(
            channels = dlc_channels.len(),
            "We have more than one DLC channel. This should not happen"
        );
    }

    let maybe_dlc_channel = dlc_channels.first();

    let status = maybe_dlc_channel.into();

    Ok(status)
}

impl From<Option<&SignedChannel>> for ChannelStatus {
    fn from(value: Option<&SignedChannel>) -> Self {
        match value {
            None => Self::NotOpen,
            Some(channel) => match channel.state {
                SignedChannelState::Established { .. } => Self::WithPosition,
                SignedChannelState::Settled { .. } | SignedChannelState::RenewFinalized { .. } => {
                    Self::Open
                }
                SignedChannelState::SettledOffered { .. }
                | SignedChannelState::SettledReceived { .. }
                | SignedChannelState::SettledAccepted { .. }
                | SignedChannelState::SettledConfirmed { .. } => Self::Settling,
                SignedChannelState::RenewOffered { .. }
                | SignedChannelState::RenewAccepted { .. }
                | SignedChannelState::RenewConfirmed { .. } => Self::Renewing,
                SignedChannelState::Closing { .. }
                | SignedChannelState::CollaborativeCloseOffered { .. } => Self::Closing,
            },
        }
    }
}

enum SubChannelState {
    Rejected,
    Opening,
    Open,
    CollabClosing,
    CollabClosed,
    ForceClosing,
    ForceClosed,
}

impl From<&SubChannel> for SubChannelState {
    fn from(value: &SubChannel) -> Self {
        use ln_dlc_node::node::rust_dlc_manager::subchannel::SubChannelState::*;
        match value.state {
            Rejected => SubChannelState::Rejected,
            Offered(_) | Accepted(_) | Confirmed(_) | Finalized(_) => SubChannelState::Opening,
            Signed(_) => SubChannelState::Open,
            CloseOffered(_) | CloseAccepted(_) | CloseConfirmed(_) => {
                SubChannelState::CollabClosing
            }
            OffChainClosed => SubChannelState::CollabClosed,
            Closing(_) => SubChannelState::ForceClosing,
            OnChainClosed | CounterOnChainClosed | ClosedPunished(_) => {
                SubChannelState::ForceClosed
            }
        }
    }
}
