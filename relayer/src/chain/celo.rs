#![allow(unused)]

use super::ChainEndpoint;
use crate::config::ChainConfig;
use crate::error::Error;
use crate::chain::{TxResponse, HealthCheck};
use crate::event::monitor::{
    EventBatch, EventReceiver, EventSender, MonitorCmd, Result as EventResult, TxMonitorCmd,
};
use crate::keyring::{KeyEntry, KeyRing, Store};
use crate::light_client::celo_ibft::LightClient as CeloLightClient;
use crate::light_client::LightClient;
use crate::light_client::Verified;
use celo_light_client::ToRlp;
use chrono::{DateTime, NaiveDateTime, Utc};
use crossbeam_channel::{Receiver, Sender};
use ibc::events::IbcEvent;
use ibc::ics02_client::client_consensus::{
    AnyConsensusState, AnyConsensusStateWithHeight, ConsensusState,
};
use ibc::ics02_client::client_state::{AnyClientState, ClientState, IdentifiedAnyClientState};
use ibc::ics02_client::client_type::ClientType;
use ibc::ics02_client::events::Attributes;
use ibc::ics02_client::events::{CreateClient, UpdateClient};
use ibc::ics02_client::header::Header;
use ibc::ics02_client::height::Height;
use ibc::ics03_connection::connection::{
    ConnectionEnd, Counterparty, IdentifiedConnectionEnd, State as ConnState,
};
use ibc::ics03_connection::events::{Attributes as ConnAttributes, OpenTry};
use ibc::ics04_channel::channel::{ChannelEnd, IdentifiedChannelEnd};
use ibc::ics04_channel::packet::{PacketMsgType, Sequence};
use ibc::ics23_commitment::commitment::{CommitmentPrefix, CommitmentRoot};
use ibc::ics24_host::identifier::{ChainId, ChannelId, ClientId, ConnectionId, PortId};
use ibc::ics28_wasm::client_state::ClientState as WasmClientState;
use ibc::ics28_wasm::consensus_state::ConsensusState as WasmConsensuState;
use ibc::ics28_wasm::header::Header as WasmHeader;
use ibc::query::QueryTxRequest;
use ibc::signer::Signer;
use ibc::Height as ICSHeight;
use ibc_proto::ibc::core::channel::v1::{
    PacketState, QueryChannelClientStateRequest, QueryChannelsRequest,
    QueryConnectionChannelsRequest, QueryNextSequenceReceiveRequest,
    QueryPacketAcknowledgementsRequest, QueryPacketCommitmentsRequest, QueryUnreceivedAcksRequest,
    QueryUnreceivedPacketsRequest,
};
use ibc_proto::ibc::core::client::v1::{QueryClientStatesRequest, QueryConsensusStatesRequest};
use ibc_proto::ibc::core::commitment::v1::MerkleProof;
use ibc_proto::ibc::core::connection::v1::{
    QueryClientConnectionsRequest, QueryConnectionsRequest,
};
use prost::Message;
use prost_types::Any;
use std::convert::From;
use std::convert::{TryFrom, TryInto};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime as TokioRuntime;

pub mod compatibility;
use compatibility::*;

pub struct CeloChain {
    config: ChainConfig,
    rt: Arc<TokioRuntime>,
    keybase: KeyRing,
    shutdown: (TxMonitorCmd, Receiver<MonitorCmd>),
    starter: (Sender<bool>, Receiver<bool>),
    blocks: Vec<SimulationBlock>,
    latest_height: Arc<AtomicU64>,
    tm_client: compatibility::tm::DummyTendermint,
}

impl CeloChain {
    fn inner_build_client_state(&self, idx: usize) -> WasmClientState {
        let block = self.blocks.get(idx).unwrap();
        let h = Height {
            revision_number: 0,
            revision_height: block.header.number.clone().try_into().unwrap(),
        };
        let cl = WasmClientState {
            chain_id: self.config.id.clone(),
            code_id: hex::decode(&self.config.code_id).unwrap(),
            data: block.initial_client_state.to_rlp(),
            is_frozen: false,
            latest_height: h,
        };
        cl
    }
    fn inner_build_consensus_state(&self, idx: usize) -> WasmConsensuState {
        let block = self.blocks.get(idx).unwrap();
        let s = WasmConsensuState {
            data: block.initial_consensus_state.to_rlp(),
            code_id: hex::decode(&self.config.code_id).unwrap(),
            root: CommitmentRoot::from_bytes(block.header.root.as_ref()),
            timestamp: DateTime::<Utc>::from_utc(
                NaiveDateTime::from_timestamp(block.initial_consensus_state.timestamp as i64, 0),
                Utc,
            ),
        };
        s
    }
}

impl ChainEndpoint for CeloChain {
    type LightBlock = SimulationBlock;
    type Header = WasmHeader;
    type ConsensusState = WasmConsensuState;
    type ClientState = WasmClientState;
    type LightClient = CeloLightClient;

    fn bootstrap(config: ChainConfig, rt: Arc<TokioRuntime>) -> Result<Self, Error> {
        println!("giulio - CeloChain::bootstrap");
        let shutdown = crossbeam_channel::unbounded::<MonitorCmd>();
        let starter = crossbeam_channel::unbounded::<bool>();
        let keybase = KeyRing::new(Store::Test, &config.account_prefix, &config.id)
            .map_err(Error::key_base)?;
        let sim_data =
            std::fs::read_to_string(std::path::Path::new(&config.simul_file)).map_err(Error::io)?;
        let mut blocks: Vec<SimulationBlock> = sim_data
            .split("\n\n")
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let s = Self {
            config,
            rt,
            keybase,
            shutdown,
            starter,
            blocks,
            latest_height: Arc::new(AtomicU64::new(0)),
            tm_client: compatibility::tm::DummyTendermint::default(),
        };
        Ok(s)
    }
    fn init_light_client(&self) -> Result<Self::LightClient, Error> {
        let lt = CeloLightClient {
            blocks: self.blocks.clone(),
        };
        Ok(lt)
    }
    fn init_event_monitor(
        &self,
        rt: Arc<TokioRuntime>,
    ) -> Result<(EventReceiver, TxMonitorCmd), Error> {
        println!("giulio - CeloChain::init_event_monitor");
        let events = crossbeam_channel::unbounded::<EventResult<EventBatch>>();
        let ch_id = self.config.id.clone();
        let starter = self.starter.1.clone();
        let tx_receiver = self.shutdown.1.clone();
        let sender = events.0;
        let blocks = self.blocks.clone();
        let counter = Arc::clone(&self.latest_height);
        rt.spawn(async move {
            match starter.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(true) => {
                    println!("giulio CeloChain::init_event_monitor - start sending ");
                }
                Ok(false) => {
                    println!("giulio CeloChain::init_event_monitor - starter false ");
                    return;
                }
                Err(_) => {
                    println!("giulio CeloChain::init_event_monitor - start ch is closed ");
                    return;
                }
            };
            std::thread::sleep(std::time::Duration::from_secs(5));
            for (idx, block) in blocks.iter().enumerate() {
                counter.store(idx as u64, Ordering::Relaxed);
                if !tx_receiver.is_empty() {
                    break;
                }
                let h = Height {
                    revision_number: 0,
                    revision_height: block.header.number.clone().try_into().unwrap(),
                };
                let wasm_h = WasmHeader {
                    height: h,
                    data: block.header.to_rlp(),
                };
                let h = wasm_h.height();
                let attr = Attributes {
                    client_id: ClientId::new(ClientType::Wasm, 0).unwrap(),
                    client_type: ClientType::Wasm,
                    consensus_height: h,
                    height: h,
                };
                let u_c = UpdateClient {
                    common: attr,
                    header: Some(wasm_h.wrap_any()),
                };
                let ev = EventBatch {
                    chain_id: ch_id.clone(),
                    height: h,
                    events: vec![IbcEvent::UpdateClient(u_c)],
                };
                println!("giulio - event_monitor - sending {}", idx);
                sender.send(Ok(ev)).unwrap();
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        Ok((events.1, self.shutdown.0.clone()))
    }
    fn shutdown(self) -> Result<(), Error> {
        println!("giulio - CeloChain::shutdown");
        self.starter.0.send(false);
        self.shutdown
            .0
            .send(MonitorCmd::Shutdown)
            .map_err(Error::send)
    }
    fn id(&self) -> &ChainId {
        println!("giulio - CeloChain::id");
        &self.config.id
    }
    fn keybase(&self) -> &KeyRing {
        println!("giulio - CeloChain::keybase");
        &self.keybase
    }
    fn keybase_mut(&mut self) -> &mut KeyRing {
        println!("giulio - CeloChain::keybase_mut");
        &mut self.keybase
    }
    fn send_messages_and_wait_check_tx(
        &mut self,
        proto_msgs: Vec<Any>,
    ) -> Result<Vec<TxResponse>, Error> {
        todo!("send_messages_and_wait_check_tx")
    }
    fn health_check(&self) -> Result<HealthCheck, Error> {
        Ok(HealthCheck::Healthy)
    }
    fn send_messages_and_wait_commit(
        &mut self,
        proto_msgs: Vec<Any>,
    ) -> Result<Vec<IbcEvent>, Error> {
        println!("giulio - CeloChain::send_msgs - {:?}", proto_msgs.len());
        let mut resps = Vec::with_capacity(proto_msgs.len());
        for msg in proto_msgs {
            match msg.type_url.as_str() {
                "/ibc.core.client.v1.MsgCreateClient" => {
                    let res = ibc_proto::ibc::core::client::v1::MsgCreateClient::decode(
                        msg.value.as_slice(),
                    )
                    .unwrap();
                    let tm_client = compatibility::tm::DummyTendermint::from_create_client_msg(res);
                    self.tm_client = tm_client;
                    let event = CreateClient(Attributes {
                        client_id: self.tm_client.client_id(),
                        client_type: ClientType::Tendermint,
                        height: self.query_latest_height().unwrap(),
                        consensus_height: self.tm_client.height(),
                    });
                    resps.push(IbcEvent::CreateClient(event));
                }
                "/ibc.core.client.v1.MsgUpdateClient" => {
                    let res = ibc_proto::ibc::core::client::v1::MsgUpdateClient::decode(
                        msg.value.as_slice(),
                    )
                    .unwrap();
                    self.tm_client.update(res);
                    let event = UpdateClient {
                        common: Attributes {
                            height: self.query_latest_height().unwrap(),
                            consensus_height: self
                                .tm_client
                                .cls
                                .last()
                                .unwrap()
                                .latest_height
                                .clone(),
                            client_type: ClientType::Tendermint,
                            client_id: self.tm_client.client_id(),
                        },
                        header: None,
                    };
                    resps.push(IbcEvent::UpdateClient(event));
                }
                "/ibc.core.connection.v1.MsgConnectionOpenTry" => {
                    let conn_try =
                        ibc_proto::ibc::core::connection::v1::MsgConnectionOpenTry::decode(
                            msg.value.as_slice(),
                        )
                        .unwrap();
                    let (id, cend) = self.tm_client.conn_try_open(conn_try.clone());
                    let resp = OpenTry::from(ConnAttributes {
                        height: self.query_latest_height().unwrap(),
                        client_id: cend.client_id().clone(),
                        connection_id: Some(id),
                        counterparty_connection_id: None,
                        counterparty_client_id: cend.counterparty().client_id().clone(),
                    });
                    resps.push(IbcEvent::OpenTryConnection(resp))
                }
                _ => panic!("typeUrl Unknown {}", msg.type_url),
            }
        }
        self.starter.0.send(true);
        Ok(resps)
    }
    fn get_signer(&mut self) -> Result<Signer, Error> {
        println!("giulio - CeloChain::get_signer");
        let s = Signer::from(String::from("this is a celo Signer"));
        Ok(s)
    }
    fn get_key(&mut self) -> Result<KeyEntry, Error> {
        println!("giulio - CeloChain::get_key");
        let key = self
            .keybase()
            .get_key(&self.config.key_name)
            .map_err(Error::key_base)?;

        Ok(key)
    }
    fn query_commitment_prefix(&self) -> Result<CommitmentPrefix, Error> {
        println!("giulio - CeloChain::query_commitment_prefix");
        Ok(CommitmentPrefix::from(
            self.config.store_prefix.as_bytes().to_vec(),
        ))
    }
    fn query_latest_height(&self) -> Result<ICSHeight, Error> {
        let idx: u64 = self.latest_height.fetch_add(1, Ordering::Relaxed);
        let block = self.blocks.get((idx + 1) as usize).unwrap();
        //let block = self.blocks.get(idx as usize).unwrap();
        let r = ICSHeight {
            revision_number: 0,
            revision_height: block.header.number.clone().try_into().unwrap(),
        };
        println!("giulio CeloChain::query_latest_height - {} - {:?}", idx, r);
        Ok(r)
    }
    fn query_clients(
        &self,
        request: QueryClientStatesRequest,
    ) -> Result<Vec<IdentifiedAnyClientState>, Error> {
        println!("giulio - CeloChain::query_clients {:?}", request);
        let idx = self.latest_height.fetch_add(1, Ordering::Relaxed);
        let cs = self.inner_build_client_state(idx as usize);

        let state = IdentifiedAnyClientState {
            client_id: ClientId::new(ClientType::Wasm, 0).unwrap(),
            client_state: cs.wrap_any(),
        };
        let states: Vec<IdentifiedAnyClientState> = vec![state];
        Ok(states)
    }
    fn query_client_state(
        &self,
        client_id: &ClientId,
        height: ICSHeight,
    ) -> Result<AnyClientState, Error> {
        println!(
            "giulio - CeloChain::query_client_state {:?} - {:?}",
            client_id, height
        );
        if *client_id == self.tm_client.client_id() {
            let cl = self.tm_client.cls.last().unwrap().clone().wrap_any();
            return Ok(cl);
        }
        let idx = self.latest_height.fetch_add(1, Ordering::Relaxed);
        let cs = self.inner_build_client_state(idx as usize);
        Ok(cs.wrap_any())
    }
    fn query_consensus_states(
        &self,
        request: QueryConsensusStatesRequest,
    ) -> Result<Vec<AnyConsensusStateWithHeight>, Error> {
        println!("giulio - CeloChain::query_consensus_states {:?}", request);

        let states = self.tm_client.consensus_states_with_height();
        Ok(states)
    }
    fn query_consensus_state(
        &self,
        client_id: ClientId,
        consensus_height: ICSHeight,
        query_height: ICSHeight,
    ) -> Result<AnyConsensusState, Error> {
        println!(
            "giulio - CeloChain;:query_consensus_state - {} {} {}",
            client_id, consensus_height, query_height
        );
        if client_id == self.tm_client.client_id() {
            let cs = self.tm_client.css.last().unwrap().clone().wrap_any();
            return Ok(cs);
        }
        panic!("what --- {:?}", client_id)
    }
    fn query_upgraded_client_state(
        &self,
        height: ICSHeight,
    ) -> Result<(Self::ClientState, MerkleProof), Error> {
        todo!()
    }
    fn query_upgraded_consensus_state(
        &self,
        height: ICSHeight,
    ) -> Result<(Self::ConsensusState, MerkleProof), Error> {
        todo!()
    }
    fn query_connections(
        &self,
        request: QueryConnectionsRequest,
    ) -> Result<Vec<IdentifiedConnectionEnd>, Error> {
        todo!()
    }
    fn query_client_connections(
        &self,
        request: QueryClientConnectionsRequest,
    ) -> Result<Vec<ConnectionId>, Error> {
        println!(
            "giulio - CeloChain::query_client_connections - {:?}",
            request
        );
        let ids = self
            .tm_client
            .conns
            .iter()
            .enumerate()
            .map(|(idx, _)| ConnectionId::new(idx as u64))
            .collect();
        Ok(ids)
    }
    fn query_connection(
        &self,
        connection_id: &ConnectionId,
        height: ICSHeight,
    ) -> Result<ConnectionEnd, Error> {
        println!(
            "giulio - CeloChain::query_connection - {:?} - {:?}",
            connection_id, height
        );
        let cend = self.tm_client.conn_end(connection_id).unwrap_or_default();
        Ok(cend)
    }
    fn query_connection_channels(
        &self,
        request: QueryConnectionChannelsRequest,
    ) -> Result<Vec<IdentifiedChannelEnd>, Error> {
        todo!()
    }
    fn query_channels(
        &self,
        request: QueryChannelsRequest,
    ) -> Result<Vec<IdentifiedChannelEnd>, Error> {
        todo!()
    }
    fn query_channel(
        &self,
        port_id: &PortId,
        channel_id: &ChannelId,
        height: ICSHeight,
    ) -> Result<ChannelEnd, Error> {
        todo!()
    }
    fn query_channel_client_state(
        &self,
        request: QueryChannelClientStateRequest,
    ) -> Result<Option<IdentifiedAnyClientState>, Error> {
        todo!()
    }
    fn query_packet_commitments(
        &self,
        request: QueryPacketCommitmentsRequest,
    ) -> Result<(Vec<PacketState>, ICSHeight), Error> {
        todo!()
    }
    fn query_unreceived_packets(
        &self,
        request: QueryUnreceivedPacketsRequest,
    ) -> Result<Vec<u64>, Error> {
        todo!()
    }
    fn query_packet_acknowledgements(
        &self,
        request: QueryPacketAcknowledgementsRequest,
    ) -> Result<(Vec<PacketState>, ICSHeight), Error> {
        todo!()
    }
    fn query_unreceived_acknowledgements(
        &self,
        request: QueryUnreceivedAcksRequest,
    ) -> Result<Vec<u64>, Error> {
        todo!()
    }
    fn query_next_sequence_receive(
        &self,
        request: QueryNextSequenceReceiveRequest,
    ) -> Result<Sequence, Error> {
        todo!()
    }
    fn query_txs(&self, request: QueryTxRequest) -> Result<Vec<IbcEvent>, Error> {
        todo!()
    }
    fn proven_client_state(
        &self,
        client_id: &ClientId,
        height: ICSHeight,
    ) -> Result<(Self::ClientState, MerkleProof), Error> {
        todo!(
            "giulio - CeloChain::proven_client_state = {:?} - {:?}",
            client_id,
            height
        );
    }
    fn proven_connection(
        &self,
        connection_id: &ConnectionId,
        height: ICSHeight,
    ) -> Result<(ConnectionEnd, MerkleProof), Error> {
        todo!(
            "giulio - CeloChain::proven_connection = {:?} - {:?}",
            connection_id,
            height
        );
    }
    fn proven_client_consensus(
        &self,
        client_id: &ClientId,
        consensus_height: ICSHeight,
        height: ICSHeight,
    ) -> Result<(Self::ConsensusState, MerkleProof), Error> {
        todo!()
    }
    fn proven_channel(
        &self,
        port_id: &PortId,
        channel_id: &ChannelId,
        height: ICSHeight,
    ) -> Result<(ChannelEnd, MerkleProof), Error> {
        todo!()
    }
    fn proven_packet(
        &self,
        packet_type: PacketMsgType,
        port_id: PortId,
        channel_id: ChannelId,
        sequence: Sequence,
        height: ICSHeight,
    ) -> Result<(Vec<u8>, MerkleProof), Error> {
        todo!()
    }

    fn build_client_state(&self, height: ICSHeight) -> Result<Self::ClientState, Error> {
        println!("giulio - CeloChain::build_client_state");
        let first_block = self.blocks.first().unwrap();
        let cl = WasmClientState {
            chain_id: self.config.id.clone(),
            code_id: hex::decode(&self.config.code_id).unwrap(),
            data: first_block.initial_client_state.to_rlp(),
            is_frozen: false,
            latest_height: height,
        };
        Ok(cl)
    }
    fn build_consensus_state(
        &self,
        block: Self::LightBlock,
    ) -> Result<Self::ConsensusState, Error> {
        println!("giulio CeloChain::build_consensus_state");
        let s = WasmConsensuState {
            data: block.initial_consensus_state.to_rlp(),
            code_id: hex::decode(&self.config.code_id).unwrap(),
            root: CommitmentRoot::from_bytes(block.header.root.as_ref()),
            timestamp: DateTime::<Utc>::from_utc(
                NaiveDateTime::from_timestamp(block.initial_consensus_state.timestamp as i64, 0),
                Utc,
            ),
        };
        Ok(s)
    }
    fn build_header(
        &self,
        trusted_height: ICSHeight,
        target_height: ICSHeight,
        client_state: &AnyClientState,
        light_client: &mut Self::LightClient,
    ) -> Result<(Self::Header, Vec<Self::Header>), Error> {
        println!("giulio CeloChain::build_header");
        let Verified { target, supporting } =
            light_client.header_and_minimal_set(trusted_height, target_height, client_state)?;

        Ok((target, supporting))
    }
}
