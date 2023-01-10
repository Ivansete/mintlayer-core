// Copyright (c) 2022 RBB S.r.l
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

use std::{net::SocketAddr, sync::Arc, time::Duration};

use tokio::{sync::oneshot, time::timeout};

use crate::testing_utils::{
    TestTransportChannel, TestTransportMaker, TestTransportNoise, TestTransportTcp,
};
use common::chain::config;

use crate::{
    error::{DialError, P2pError, ProtocolError},
    event::PeerManagerEvent,
    net::{
        self,
        mock::{
            transport::{MockChannelTransport, NoiseTcpTransport, TcpTransportSocket},
            types::MockPeerId,
            MockService,
        },
        types::PubSubTopic,
        ConnectivityService, NetworkingService,
    },
    peer_manager::{self, helpers::connect_services, tests::make_peer_manager},
};

// try to connect to an address that no one listening on and verify it fails
async fn test_peer_manager_connect<T: NetworkingService>(
    transport: T::Transport,
    bind_addr: T::Address,
    remote_addr: T::Address,
) where
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let config = Arc::new(config::create_mainnet());
    let mut peer_manager = make_peer_manager::<T>(transport, bind_addr, config).await;

    peer_manager.connect(remote_addr).await.unwrap();

    assert!(matches!(
        peer_manager.peer_connectivity_handle.poll_next().await,
        Ok(net::types::ConnectivityEvent::ConnectionError {
            address: _,
            error: P2pError::DialError(DialError::ConnectionRefusedOrTimedOut)
        })
    ));
}

#[tokio::test]
async fn test_peer_manager_connect_mock() {
    let transport = TestTransportTcp::make_transport();
    let bind_addr = TestTransportTcp::make_address();
    let remote_addr: SocketAddr = "[::1]:1".parse().unwrap();

    test_peer_manager_connect::<MockService<TcpTransportSocket>>(transport, bind_addr, remote_addr)
        .await;
}

// verify that the auto-connect functionality works if the number of active connections
// is below the desired threshold and there are idle peers in the peerdb
async fn test_auto_connect<A, T>(peer_id: T::PeerId)
where
    A: TestTransportMaker<Transport = T::Transport, Address = T::Address>,
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let addr1 = A::make_address();
    let addr2 = A::make_address();

    let config = Arc::new(config::create_mainnet());
    let mut pm1 = make_peer_manager::<T>(A::make_transport(), addr1, Arc::clone(&config)).await;
    let mut pm2 = make_peer_manager::<T>(A::make_transport(), addr2, config).await;

    let addr = pm2.peer_connectivity_handle.local_addr().await.unwrap().unwrap();

    tokio::spawn(async move {
        loop {
            assert!(pm2.peer_connectivity_handle.poll_next().await.is_ok());
        }
    });

    // "discover" the other networking service
    pm1.peer_discovered(&[net::types::AddrInfo {
        peer_id,
        ip4: vec![],
        ip6: vec![addr],
    }]);
    pm1.heartbeat().await.unwrap();

    assert_eq!(pm1.pending.len(), 1);
    assert!(std::matches!(
        pm1.peer_connectivity_handle.poll_next().await,
        Ok(net::types::ConnectivityEvent::OutboundAccepted { .. })
    ));
}

#[tokio::test]
async fn test_auto_connect_mock_tcp() {
    test_auto_connect::<TestTransportTcp, MockService<TcpTransportSocket>>(MockPeerId::new()).await;
}

#[tokio::test]
async fn test_auto_connect_mock_channels() {
    test_auto_connect::<TestTransportChannel, MockService<MockChannelTransport>>(MockPeerId::new())
        .await;
}

#[tokio::test]
async fn test_auto_connect_mock_noise() {
    test_auto_connect::<TestTransportNoise, MockService<NoiseTcpTransport>>(MockPeerId::new())
        .await;
}

async fn connect_outbound_same_network<A, T>()
where
    A: TestTransportMaker<Transport = T::Transport, Address = T::Address>,
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let addr1 = A::make_address();
    let addr2 = A::make_address();

    let config = Arc::new(config::create_mainnet());
    let mut pm1 = make_peer_manager::<T>(A::make_transport(), addr1, Arc::clone(&config)).await;
    let mut pm2 = make_peer_manager::<T>(A::make_transport(), addr2, config).await;

    connect_services::<T>(
        &mut pm1.peer_connectivity_handle,
        &mut pm2.peer_connectivity_handle,
    )
    .await;
}

#[tokio::test]
async fn connect_outbound_same_network_mock_tcp() {
    connect_outbound_same_network::<TestTransportTcp, MockService<TcpTransportSocket>>().await;
}

#[tokio::test]
async fn connect_outbound_same_network_mock_channels() {
    connect_outbound_same_network::<TestTransportChannel, MockService<MockChannelTransport>>()
        .await;
}

#[tokio::test]
async fn connect_outbound_same_network_mock_noise() {
    connect_outbound_same_network::<TestTransportNoise, MockService<NoiseTcpTransport>>().await;
}

async fn connect_outbound_different_network<A, T>()
where
    A: TestTransportMaker<Transport = T::Transport, Address = T::Address>,
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let addr1 = A::make_address();
    let addr2 = A::make_address();

    let config = Arc::new(config::create_mainnet());
    let mut pm1 = make_peer_manager::<T>(A::make_transport(), addr1, Arc::clone(&config)).await;
    let mut pm2 = make_peer_manager::<T>(
        A::make_transport(),
        addr2,
        Arc::new(common::chain::config::Builder::test_chain().magic_bytes([1, 2, 3, 4]).build()),
    )
    .await;

    let (_address, peer_info, _) = connect_services::<T>(
        &mut pm2.peer_connectivity_handle,
        &mut pm1.peer_connectivity_handle,
    )
    .await;
    assert_ne!(peer_info.magic_bytes, *config.magic_bytes());
}

#[tokio::test]
async fn connect_outbound_different_network_mock_tcp() {
    connect_outbound_different_network::<TestTransportTcp, MockService<TcpTransportSocket>>().await;
}

#[tokio::test]
async fn connect_outbound_different_network_mock_channels() {
    connect_outbound_different_network::<TestTransportChannel, MockService<MockChannelTransport>>()
        .await;
}

#[tokio::test]
async fn connect_outbound_different_network_mock_noise() {
    connect_outbound_different_network::<TestTransportNoise, MockService<NoiseTcpTransport>>()
        .await;
}

async fn connect_inbound_same_network<A, T>()
where
    A: TestTransportMaker<Transport = T::Transport, Address = T::Address>,
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let addr1 = A::make_address();
    let addr2 = A::make_address();

    let config = Arc::new(config::create_mainnet());
    let mut pm1 = make_peer_manager::<T>(A::make_transport(), addr1, Arc::clone(&config)).await;
    let mut pm2 = make_peer_manager::<T>(A::make_transport(), addr2, config).await;

    let (address, peer_info, _) = connect_services::<T>(
        &mut pm1.peer_connectivity_handle,
        &mut pm2.peer_connectivity_handle,
    )
    .await;
    assert_eq!(pm2.accept_inbound_connection(address, peer_info), Ok(()));
}

#[tokio::test]
async fn connect_inbound_same_network_mock_tcp() {
    connect_inbound_same_network::<TestTransportTcp, MockService<TcpTransportSocket>>().await;
}

#[tokio::test]
async fn connect_inbound_same_network_mock_channel() {
    connect_inbound_same_network::<TestTransportChannel, MockService<MockChannelTransport>>().await;
}

#[tokio::test]
async fn connect_inbound_same_network_mock_noise() {
    connect_inbound_same_network::<TestTransportNoise, MockService<NoiseTcpTransport>>().await;
}

async fn connect_inbound_different_network<A, T>()
where
    A: TestTransportMaker<Transport = T::Transport, Address = T::Address>,
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let addr1 = A::make_address();
    let addr2 = A::make_address();

    let mut pm1 = make_peer_manager::<T>(
        A::make_transport(),
        addr1,
        Arc::new(config::create_mainnet()),
    )
    .await;
    let mut pm2 = make_peer_manager::<T>(
        A::make_transport(),
        addr2,
        Arc::new(common::chain::config::Builder::test_chain().magic_bytes([1, 2, 3, 4]).build()),
    )
    .await;

    let (address, peer_info, _) = connect_services::<T>(
        &mut pm1.peer_connectivity_handle,
        &mut pm2.peer_connectivity_handle,
    )
    .await;

    assert_eq!(
        pm2.accept_inbound_connection(address, peer_info),
        Err(P2pError::ProtocolError(ProtocolError::DifferentNetwork(
            [1, 2, 3, 4],
            *config::create_mainnet().magic_bytes(),
        )))
    );
}

#[tokio::test]
async fn connect_inbound_different_network_mock_tcp() {
    connect_inbound_different_network::<TestTransportTcp, MockService<TcpTransportSocket>>().await;
}

#[tokio::test]
async fn connect_inbound_different_network_mock_channels() {
    connect_inbound_different_network::<TestTransportChannel, MockService<MockChannelTransport>>()
        .await;
}

#[tokio::test]
async fn connect_inbound_different_network_mock_noise() {
    connect_inbound_different_network::<TestTransportNoise, MockService<NoiseTcpTransport>>().await;
}

async fn remote_closes_connection<A, T>()
where
    A: TestTransportMaker<Transport = T::Transport, Address = T::Address>,
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let addr1 = A::make_address();
    let addr2 = A::make_address();

    let mut pm1 = make_peer_manager::<T>(
        A::make_transport(),
        addr1,
        Arc::new(config::create_mainnet()),
    )
    .await;
    let mut pm2 = make_peer_manager::<T>(
        A::make_transport(),
        addr2,
        Arc::new(config::create_mainnet()),
    )
    .await;

    let (_address, peer_info, _) = connect_services::<T>(
        &mut pm1.peer_connectivity_handle,
        &mut pm2.peer_connectivity_handle,
    )
    .await;

    assert_eq!(
        pm2.peer_connectivity_handle.disconnect(peer_info.peer_id).await,
        Ok(())
    );
    assert!(std::matches!(
        pm1.peer_connectivity_handle.poll_next().await,
        Ok(net::types::ConnectivityEvent::ConnectionClosed { .. })
    ));
}

#[tokio::test]
async fn remote_closes_connection_mock_tcp() {
    remote_closes_connection::<TestTransportTcp, MockService<TcpTransportSocket>>().await;
}

#[tokio::test]
async fn remote_closes_connection_mock_channels() {
    remote_closes_connection::<TestTransportChannel, MockService<MockChannelTransport>>().await;
}

#[tokio::test]
async fn remote_closes_connection_mock_noise() {
    remote_closes_connection::<TestTransportNoise, MockService<NoiseTcpTransport>>().await;
}

async fn inbound_connection_too_many_peers<A, T>(peers: Vec<net::types::PeerInfo<T>>)
where
    A: TestTransportMaker<Transport = T::Transport, Address = T::Address>,
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let addr1 = A::make_address();
    let addr2 = A::make_address();
    let default_addr = A::make_address();

    let config = Arc::new(config::create_mainnet());
    let mut pm1 = make_peer_manager::<T>(A::make_transport(), addr1, Arc::clone(&config)).await;
    let mut pm2 = make_peer_manager::<T>(A::make_transport(), addr2, Arc::clone(&config)).await;

    peers.into_iter().for_each(|info| {
        pm1.peerdb.peer_connected(default_addr.clone(), info);
    });
    assert_eq!(
        pm1.peerdb.active_peer_count(),
        peer_manager::MAX_ACTIVE_CONNECTIONS
    );

    let (_address, peer_info, _) = connect_services::<T>(
        &mut pm1.peer_connectivity_handle,
        &mut pm2.peer_connectivity_handle,
    )
    .await;

    // run the first peer manager in the background and poll events from the peer manager
    // that tries to connect to the first manager
    tokio::spawn(async move { pm1.run().await });

    if let Ok(net::types::ConnectivityEvent::ConnectionClosed { peer_id }) =
        pm2.peer_connectivity_handle.poll_next().await
    {
        assert_eq!(peer_id, peer_info.peer_id);
    } else {
        panic!("invalid event received");
    }
}

#[tokio::test]
async fn inbound_connection_too_many_peers_mock_tcp() {
    let config = Arc::new(config::create_mainnet());
    let peers = (0..peer_manager::MAX_ACTIVE_CONNECTIONS)
        .map(
            |_| net::types::PeerInfo::<MockService<TcpTransportSocket>> {
                peer_id: MockPeerId::new(),
                magic_bytes: *config.magic_bytes(),
                version: common::primitives::semver::SemVer::new(0, 1, 0),
                agent: None,
                subscriptions: [PubSubTopic::Blocks, PubSubTopic::Transactions]
                    .into_iter()
                    .collect(),
            },
        )
        .collect::<Vec<_>>();

    inbound_connection_too_many_peers::<TestTransportTcp, MockService<TcpTransportSocket>>(peers)
        .await;
}

#[tokio::test]
async fn inbound_connection_too_many_peers_mock_channels() {
    let config = Arc::new(config::create_mainnet());
    let peers = (0..peer_manager::MAX_ACTIVE_CONNECTIONS)
        .map(
            |_| net::types::PeerInfo::<MockService<MockChannelTransport>> {
                peer_id: MockPeerId::new(),
                magic_bytes: *config.magic_bytes(),
                version: common::primitives::semver::SemVer::new(0, 1, 0),
                agent: None,
                subscriptions: [PubSubTopic::Blocks, PubSubTopic::Transactions]
                    .into_iter()
                    .collect(),
            },
        )
        .collect::<Vec<_>>();

    inbound_connection_too_many_peers::<TestTransportChannel, MockService<MockChannelTransport>>(
        peers,
    )
    .await;
}

#[tokio::test]
async fn inbound_connection_too_many_peers_mock_noise() {
    let config = Arc::new(config::create_mainnet());
    let peers = (0..peer_manager::MAX_ACTIVE_CONNECTIONS)
        .map(|_| net::types::PeerInfo::<MockService<NoiseTcpTransport>> {
            peer_id: MockPeerId::new(),
            magic_bytes: *config.magic_bytes(),
            version: common::primitives::semver::SemVer::new(0, 1, 0),
            agent: None,
            subscriptions: [PubSubTopic::Blocks, PubSubTopic::Transactions].into_iter().collect(),
        })
        .collect::<Vec<_>>();

    inbound_connection_too_many_peers::<TestTransportNoise, MockService<NoiseTcpTransport>>(peers)
        .await;
}

async fn connection_timeout<T>(transport: T::Transport, addr1: T::Address, addr2: T::Address)
where
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let config = Arc::new(config::create_mainnet());
    let mut pm1 = make_peer_manager::<T>(transport, addr1, Arc::clone(&config)).await;

    pm1.peer_connectivity_handle.connect(addr2).await.expect("dial to succeed");

    match timeout(
        Duration::from_secs(*pm1._p2p_config.outbound_connection_timeout),
        pm1.peer_connectivity_handle.poll_next(),
    )
    .await
    {
        Ok(res) => assert!(std::matches!(
            res,
            Ok(net::types::ConnectivityEvent::ConnectionError {
                address: _,
                error: P2pError::DialError(DialError::ConnectionRefusedOrTimedOut),
            })
        )),
        Err(_err) => panic!("did not receive `ConnectionError` in time"),
    }
}

#[tokio::test]
async fn connection_timeout_mock_tcp() {
    connection_timeout::<MockService<TcpTransportSocket>>(
        TestTransportTcp::make_transport(),
        TestTransportTcp::make_address(),
        TestTransportTcp::make_address(),
    )
    .await;
}

#[tokio::test]
async fn connection_timeout_mock_channels() {
    connection_timeout::<MockService<MockChannelTransport>>(
        TestTransportChannel::make_transport(),
        TestTransportChannel::make_address(),
        65_535,
    )
    .await;
}

#[tokio::test]
async fn connection_timeout_mock_noise() {
    connection_timeout::<MockService<NoiseTcpTransport>>(
        TestTransportNoise::make_transport(),
        TestTransportNoise::make_address(),
        TestTransportNoise::make_address(),
    )
    .await;
}

// try to establish a new connection through RPC and verify that it is notified of the timeout
async fn connection_timeout_rpc_notified<T>(
    transport: T::Transport,
    addr1: T::Address,
    addr2: T::Address,
) where
    T: NetworkingService + 'static + std::fmt::Debug,
    T::ConnectivityHandle: ConnectivityService<T>,
    <T as NetworkingService>::Address: std::str::FromStr,
    <<T as NetworkingService>::Address as std::str::FromStr>::Err: std::fmt::Debug,
{
    let config = Arc::new(config::create_mainnet());
    let p2p_config = Arc::new(Default::default());
    let (conn, _) = T::start(
        transport,
        addr1,
        Arc::clone(&config),
        Arc::clone(&p2p_config),
    )
    .await
    .unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let (tx_sync, mut rx_sync) = tokio::sync::mpsc::unbounded_channel();

    let mut peer_manager = peer_manager::PeerManager::<T>::new(
        Arc::clone(&config),
        Arc::clone(&p2p_config),
        conn,
        rx,
        tx_sync,
    );

    tokio::spawn(async move { while rx_sync.recv().await.is_some() {} });
    tokio::spawn(async move {
        peer_manager.run().await.unwrap();
    });

    let (rtx, rrx) = oneshot::channel();
    tx.send(PeerManagerEvent::Connect(addr2, rtx)).unwrap();

    match timeout(
        Duration::from_secs(*p2p_config.outbound_connection_timeout),
        rrx,
    )
    .await
    {
        Ok(res) => assert!(std::matches!(
            res.unwrap(),
            Err(P2pError::DialError(DialError::ConnectionRefusedOrTimedOut))
        )),
        Err(_err) => panic!("did not receive `ConnectionError` in time"),
    }
}

#[tokio::test]
async fn connection_timeout_rpc_notified_mock_tcp() {
    connection_timeout_rpc_notified::<MockService<TcpTransportSocket>>(
        TestTransportTcp::make_transport(),
        TestTransportTcp::make_address(),
        TestTransportTcp::make_address(),
    )
    .await;
}

#[tokio::test]
async fn connection_timeout_rpc_notified_mock_channels() {
    connection_timeout_rpc_notified::<MockService<MockChannelTransport>>(
        TestTransportChannel::make_transport(),
        TestTransportChannel::make_address(),
        9999,
    )
    .await;
}

#[tokio::test]
async fn connection_timeout_rpc_notified_mock_noise() {
    connection_timeout_rpc_notified::<MockService<NoiseTcpTransport>>(
        TestTransportNoise::make_transport(),
        TestTransportNoise::make_address(),
        TestTransportNoise::make_address(),
    )
    .await;
}
