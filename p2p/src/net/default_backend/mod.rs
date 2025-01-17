// Copyright (c) 2021-2022 RBB S.r.l
// opensource@mintlayer.org
// SPDX-License-Identifier: MIT
// Licensed under the MIT License;
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://github.com/mintlayer/mintlayer-core/blob/master/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod backend;
pub mod constants;
pub mod peer;
pub mod request_manager;
pub mod transport;
pub mod types;

use std::{marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use tokio::sync::mpsc;

use logging::log;
use serialization::Encode;

use crate::{
    config,
    error::{P2pError, PublishError},
    message::{self, PeerManagerRequest, PeerManagerResponse, SyncRequest, SyncResponse},
    net::{
        default_backend::{
            constants::ANNOUNCEMENT_MAX_SIZE,
            transport::{TransportListener, TransportSocket},
            types::{PeerId, RequestId},
        },
        types::{ConnectivityEvent, PubSubTopic, SyncingEvent},
        ConnectivityService, NetworkingService, SyncingMessagingService,
    },
};

#[derive(Debug)]
pub struct DefaultNetworkingService<T: TransportSocket>(PhantomData<T>);

#[derive(Debug)]
pub struct ConnectivityHandle<S: NetworkingService, T: TransportSocket> {
    /// The local addresses of a network service provider.
    local_addresses: Vec<S::Address>,

    /// TX channel for sending commands to default_backend backend
    cmd_tx: mpsc::UnboundedSender<types::Command<T>>,

    /// RX channel for receiving connectivity events from default_backend backend
    conn_rx: mpsc::UnboundedReceiver<types::ConnectivityEvent<T>>,

    _marker: PhantomData<fn() -> S>,
}

impl<S: NetworkingService, T: TransportSocket> ConnectivityHandle<S, T> {
    pub fn new(
        local_addresses: Vec<S::Address>,
        cmd_tx: mpsc::UnboundedSender<types::Command<T>>,
        conn_rx: mpsc::UnboundedReceiver<types::ConnectivityEvent<T>>,
    ) -> Self {
        Self {
            local_addresses,
            cmd_tx,
            conn_rx,
            _marker: PhantomData,
        }
    }
}

pub struct PubSubHandle<S, T>
where
    S: NetworkingService,
    T: TransportSocket,
{
    /// TX channel for sending commands to default_backend backend
    _cmd_tx: mpsc::UnboundedSender<types::Command<T>>,

    /// RX channel for receiving pubsub events from default_backend backend
    _pubsub_rx: mpsc::UnboundedReceiver<types::PubSubEvent<T>>,

    _marker: PhantomData<fn() -> S>,
}

#[derive(Debug)]
pub struct SyncingMessagingHandle<S, T>
where
    S: NetworkingService,
    T: TransportSocket,
{
    /// TX channel for sending commands to default_backend backend
    cmd_tx: mpsc::UnboundedSender<types::Command<T>>,

    /// RX channel for receiving syncing events
    sync_rx: mpsc::UnboundedReceiver<types::SyncingEvent>,

    _marker: PhantomData<fn() -> S>,
}

#[async_trait]
impl<T: TransportSocket> NetworkingService for DefaultNetworkingService<T> {
    type Transport = T;
    type Address = T::Address;
    type BannableAddress = T::BannableAddress;
    type PeerId = PeerId;
    type PeerRequestId = RequestId;
    type ConnectivityHandle = ConnectivityHandle<Self, T>;
    type SyncingMessagingHandle = SyncingMessagingHandle<Self, T>;

    async fn start(
        transport: Self::Transport,
        bind_addresses: Vec<Self::Address>,
        chain_config: Arc<common::chain::ChainConfig>,
        p2p_config: Arc<config::P2pConfig>,
    ) -> crate::Result<(Self::ConnectivityHandle, Self::SyncingMessagingHandle)> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        let (sync_tx, sync_rx) = mpsc::unbounded_channel();
        let socket = transport.bind(bind_addresses).await?;
        let local_addresses = socket.local_addresses().expect("to have bind address available");

        tokio::spawn(async move {
            let mut backend = backend::Backend::<T>::new(
                transport,
                socket,
                chain_config,
                p2p_config,
                cmd_rx,
                conn_tx,
                sync_tx,
            );

            if let Err(err) = backend.run().await {
                log::error!("failed to run backend: {err}");
            }
        });

        Ok((
            ConnectivityHandle::new(local_addresses, cmd_tx.clone(), conn_rx),
            Self::SyncingMessagingHandle {
                cmd_tx,
                sync_rx,
                _marker: Default::default(),
            },
        ))
    }
}

#[async_trait]
impl<S, T> ConnectivityService<S> for ConnectivityHandle<S, T>
where
    S: NetworkingService<Address = T::Address, PeerId = PeerId, PeerRequestId = RequestId> + Send,
    T: TransportSocket,
{
    fn connect(&mut self, address: S::Address) -> crate::Result<()> {
        log::debug!(
            "try to establish outbound connection, address {:?}",
            address
        );

        self.cmd_tx.send(types::Command::Connect { address }).map_err(P2pError::from)
    }

    fn disconnect(&mut self, peer_id: S::PeerId) -> crate::Result<()> {
        log::debug!("close connection with remote, {peer_id}");

        self.cmd_tx.send(types::Command::Disconnect { peer_id }).map_err(P2pError::from)
    }

    fn send_request(
        &mut self,
        peer_id: S::PeerId,
        request: PeerManagerRequest,
    ) -> crate::Result<S::PeerRequestId> {
        let request_id = RequestId::new();

        self.cmd_tx.send(types::Command::SendRequest {
            peer_id,
            request_id,
            message: request.into(),
        })?;

        Ok(request_id)
    }

    fn send_response(
        &mut self,
        request_id: S::PeerRequestId,
        response: PeerManagerResponse,
    ) -> crate::Result<()> {
        self.cmd_tx
            .send(types::Command::SendResponse {
                request_id,
                message: response.into(),
            })
            .map_err(P2pError::from)
    }

    fn local_addresses(&self) -> &[S::Address] {
        &self.local_addresses
    }

    async fn poll_next(&mut self) -> crate::Result<ConnectivityEvent<S>> {
        match self.conn_rx.recv().await.ok_or(P2pError::ChannelClosed)? {
            types::ConnectivityEvent::Request {
                peer_id,
                request_id,
                request,
            } => Ok(ConnectivityEvent::Request {
                peer_id,
                request_id,
                request,
            }),
            types::ConnectivityEvent::Response {
                peer_id,
                request_id,
                response,
            } => Ok(ConnectivityEvent::Response {
                peer_id,
                request_id,
                response,
            }),
            types::ConnectivityEvent::InboundAccepted {
                address,
                peer_info,
                receiver_address,
            } => Ok(ConnectivityEvent::InboundAccepted {
                address,
                peer_info,
                receiver_address,
            }),
            types::ConnectivityEvent::OutboundAccepted {
                address,
                peer_info,
                receiver_address,
            } => Ok(ConnectivityEvent::OutboundAccepted {
                address,
                peer_info,
                receiver_address,
            }),
            types::ConnectivityEvent::ConnectionError { address, error } => {
                Ok(ConnectivityEvent::ConnectionError { address, error })
            }
            types::ConnectivityEvent::ConnectionClosed { peer_id } => {
                Ok(ConnectivityEvent::ConnectionClosed { peer_id })
            }
            types::ConnectivityEvent::Misbehaved { peer_id, error } => {
                Ok(ConnectivityEvent::Misbehaved { peer_id, error })
            }
        }
    }
}

#[async_trait]
impl<S, T> SyncingMessagingService<S> for SyncingMessagingHandle<S, T>
where
    S: NetworkingService<PeerId = PeerId, PeerRequestId = RequestId> + Send,
    T: TransportSocket,
{
    fn send_request(
        &mut self,
        peer_id: S::PeerId,
        request: SyncRequest,
    ) -> crate::Result<S::PeerRequestId> {
        let request_id = RequestId::new();

        self.cmd_tx.send(types::Command::SendRequest {
            peer_id,
            request_id,
            message: request.into(),
        })?;

        Ok(request_id)
    }

    fn send_response(
        &mut self,
        request_id: S::PeerRequestId,
        response: SyncResponse,
    ) -> crate::Result<()> {
        self.cmd_tx.send(types::Command::SendResponse {
            request_id,
            message: response.into(),
        })?;
        Ok(())
    }

    fn make_announcement(&mut self, announcement: message::Announcement) -> crate::Result<()> {
        let message = announcement.encode();
        if message.len() > ANNOUNCEMENT_MAX_SIZE {
            return Err(P2pError::PublishError(PublishError::MessageTooLarge(
                message.len(),
                ANNOUNCEMENT_MAX_SIZE,
            )));
        }

        let topic = match &announcement {
            message::Announcement::Block(_) => PubSubTopic::Blocks,
        };

        self.cmd_tx
            .send(types::Command::AnnounceData { topic, message })
            .map_err(P2pError::from)
    }

    async fn poll_next(&mut self) -> crate::Result<SyncingEvent<S>> {
        match self.sync_rx.recv().await.ok_or(P2pError::ChannelClosed)? {
            types::SyncingEvent::Request {
                peer_id,
                request_id,
                request,
            } => Ok(SyncingEvent::Request {
                peer_id,
                request_id,
                request,
            }),
            types::SyncingEvent::Response {
                peer_id,
                request_id,
                response,
            } => Ok(SyncingEvent::Response {
                peer_id,
                request_id,
                response,
            }),
            types::SyncingEvent::Announcement {
                peer_id,
                announcement,
            } => Ok(SyncingEvent::Announcement {
                peer_id,
                announcement: *announcement,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{transport::NoiseTcpTransport, *};
    use crate::error::DialError;
    use crate::testing_utils::{TestTransportChannel, TestTransportMaker, TestTransportTcp};
    use crate::{
        net::default_backend::transport::{MpscChannelTransport, TcpTransportSocket},
        testing_utils::TestTransportNoise,
    };
    use common::primitives::semver::SemVer;
    use std::fmt::Debug;

    async fn connect_to_remote<A, T>()
    where
        A: TestTransportMaker<Transport = T, Address = T::Address>,
        T: TransportSocket + Debug,
    {
        let config = Arc::new(common::chain::config::create_mainnet());
        let p2p_config: Arc<config::P2pConfig> = Arc::new(Default::default());

        let (mut conn1, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            Arc::clone(&config),
            Arc::clone(&p2p_config),
        )
        .await
        .unwrap();

        let (conn2, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            Arc::clone(&config),
            Arc::clone(&p2p_config),
        )
        .await
        .unwrap();

        let addr = conn2.local_addresses();
        assert_eq!(conn1.connect(addr[0].clone()), Ok(()));

        if let Ok(ConnectivityEvent::OutboundAccepted {
            address,
            peer_info,
            receiver_address: _,
        }) = conn1.poll_next().await
        {
            assert_eq!(address, conn2.local_addresses()[0]);
            assert_eq!(&peer_info.network, config.magic_bytes());
            assert_eq!(peer_info.version, SemVer::new(0, 1, 0));
            assert_eq!(peer_info.agent, None);
            assert_eq!(
                peer_info.subscriptions,
                [PubSubTopic::Blocks, PubSubTopic::Transactions].into_iter().collect()
            );
        } else {
            panic!("invalid event received");
        }
    }

    #[tokio::test]
    async fn connect_to_remote_tcp() {
        connect_to_remote::<TestTransportTcp, TcpTransportSocket>().await;
    }

    #[tokio::test]
    async fn connect_to_remote_channels() {
        connect_to_remote::<TestTransportChannel, MpscChannelTransport>().await;
    }

    #[tokio::test]
    async fn connect_to_remote_noise() {
        connect_to_remote::<TestTransportNoise, NoiseTcpTransport>().await;
    }

    async fn accept_incoming<A, T>()
    where
        A: TestTransportMaker<Transport = T, Address = T::Address>,
        T: TransportSocket,
    {
        let config = Arc::new(common::chain::config::create_mainnet());
        let p2p_config: Arc<config::P2pConfig> = Arc::new(Default::default());

        let (mut conn1, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            Arc::clone(&config),
            Arc::clone(&p2p_config),
        )
        .await
        .unwrap();

        let (mut conn2, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            Arc::clone(&config),
            Arc::clone(&p2p_config),
        )
        .await
        .unwrap();

        let bind_address = conn2.local_addresses();
        conn1.connect(bind_address[0].clone()).unwrap();
        let res2 = conn2.poll_next().await;
        match res2.unwrap() {
            ConnectivityEvent::InboundAccepted {
                address: _,
                peer_info,
                receiver_address: _,
            } => {
                assert_eq!(peer_info.network, *config.magic_bytes());
                assert_eq!(
                    peer_info.version,
                    common::primitives::semver::SemVer::new(0, 1, 0),
                );
                assert_eq!(peer_info.agent, None);
            }
            _ => panic!("invalid event received, expected incoming connection"),
        }
    }

    #[tokio::test]
    async fn accept_incoming_tcp() {
        accept_incoming::<TestTransportTcp, TcpTransportSocket>().await;
    }

    #[tokio::test]
    async fn accept_incoming_channels() {
        accept_incoming::<TestTransportChannel, MpscChannelTransport>().await;
    }

    #[tokio::test]
    async fn accept_incoming_noise() {
        accept_incoming::<TestTransportNoise, NoiseTcpTransport>().await;
    }

    async fn disconnect<A, T>()
    where
        A: TestTransportMaker<Transport = T, Address = T::Address>,
        T: TransportSocket,
    {
        let config = Arc::new(common::chain::config::create_mainnet());
        let p2p_config: Arc<config::P2pConfig> = Arc::new(Default::default());

        let (mut conn1, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            Arc::clone(&config),
            Arc::clone(&p2p_config),
        )
        .await
        .unwrap();
        let (mut conn2, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            config,
            p2p_config,
        )
        .await
        .unwrap();

        conn1.connect(conn2.local_addresses()[0].clone()).unwrap();
        let res2 = conn2.poll_next().await;

        match res2.unwrap() {
            ConnectivityEvent::InboundAccepted {
                address: _,
                peer_info,
                receiver_address: _,
            } => {
                assert_eq!(conn2.disconnect(peer_info.peer_id), Ok(()));
            }
            _ => panic!("invalid event received, expected incoming connection"),
        }
    }

    #[tokio::test]
    async fn disconnect_tcp() {
        disconnect::<TestTransportTcp, TcpTransportSocket>().await;
    }

    #[tokio::test]
    async fn disconnect_channels() {
        disconnect::<TestTransportChannel, MpscChannelTransport>().await;
    }

    #[tokio::test]
    async fn disconnect_noise() {
        disconnect::<TestTransportNoise, NoiseTcpTransport>().await;
    }

    async fn self_connect<A, T>()
    where
        A: TestTransportMaker<Transport = T, Address = T::Address>,
        T: TransportSocket + Debug,
    {
        let config = Arc::new(common::chain::config::create_mainnet());
        let p2p_config: Arc<config::P2pConfig> = Arc::new(Default::default());

        let (mut conn1, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            Arc::clone(&config),
            Arc::clone(&p2p_config),
        )
        .await
        .unwrap();

        let (conn2, _) = DefaultNetworkingService::<T>::start(
            A::make_transport(),
            vec![A::make_address()],
            Arc::clone(&config),
            Arc::clone(&p2p_config),
        )
        .await
        .unwrap();

        // Try connect to self
        let addr = conn1.local_addresses();
        assert_eq!(conn1.connect(addr[0].clone()), Ok(()));

        // ConnectionError should be reported
        if let Ok(ConnectivityEvent::ConnectionError { address, error }) = conn1.poll_next().await {
            assert_eq!(address, conn1.local_addresses()[0]);
            assert_eq!(error, P2pError::DialError(DialError::AttemptToDialSelf));
        } else {
            panic!("invalid event received");
        }

        // Two ConnectionClosed will be also reported
        if let Ok(ConnectivityEvent::ConnectionClosed { peer_id: _ }) = conn1.poll_next().await {
        } else {
            panic!("invalid event received");
        }
        if let Ok(ConnectivityEvent::ConnectionClosed { peer_id: _ }) = conn1.poll_next().await {
        } else {
            panic!("invalid event received");
        }

        // Check that we can still connect normally after
        let addr = conn2.local_addresses();
        assert_eq!(conn1.connect(addr[0].clone()), Ok(()));
        if let Ok(ConnectivityEvent::OutboundAccepted {
            address,
            peer_info,
            receiver_address: _,
        }) = conn1.poll_next().await
        {
            assert_eq!(address, conn2.local_addresses()[0]);
            assert_eq!(&peer_info.network, config.magic_bytes());
            assert_eq!(peer_info.version, SemVer::new(0, 1, 0));
            assert_eq!(peer_info.agent, None);
            assert_eq!(
                peer_info.subscriptions,
                [PubSubTopic::Blocks, PubSubTopic::Transactions].into_iter().collect()
            );
        } else {
            panic!("invalid event received");
        }
    }

    #[tokio::test]
    async fn self_connect_tcp() {
        self_connect::<TestTransportTcp, TcpTransportSocket>().await;
    }

    #[tokio::test]
    async fn self_connect_channels() {
        self_connect::<TestTransportChannel, MpscChannelTransport>().await;
    }

    #[tokio::test]
    async fn self_connect_noise() {
        self_connect::<TestTransportNoise, NoiseTcpTransport>().await;
    }
}
