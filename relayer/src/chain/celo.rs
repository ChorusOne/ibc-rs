#![allow(unused)]

use super::{ChainEndpoint, MinimalChainEndpoint};
use crate::chain::{HealthCheck, TxResponse};
use crate::config::CeloChainConfig;
use crate::error::Error;
use crate::event::monitor::{
    Error as EventError, EventBatch, EventReceiver, EventSender, MonitorCmd, Result as EventResult,
    TxMonitorCmd,
};
use crate::keyring::{KeyEntry, KeyRing, Store};
use crate::light_client::celo_ibft::LightClient as CeloLightClient;
use crate::light_client::{LightClient, Verified};
use crossbeam_channel::{Receiver, Sender};
use futures::TryStreamExt;
use ibc::events::IbcEvent;
use ibc::ics02_client::client_consensus::{
    AnyConsensusState, AnyConsensusStateWithHeight, ConsensusState,
};
use ibc::ics02_client::client_state::{AnyClientState, ClientState, IdentifiedAnyClientState};
use ibc::ics02_client::client_type::ClientType;
use ibc::ics02_client::events::{Attributes, CreateClient, NewBlock, UpdateClient};
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
use ibc::ics28_wasm::consensus_state::ConsensusState as WasmConsensusState;
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

pub(crate) mod compatibility;
mod contracts;
use compatibility::*;

pub struct CeloChain {
    config: CeloChainConfig,
    rt: Arc<TokioRuntime>,
    keybase: KeyRing,
    celo: Web3Client,
    client_contract: EthContract,
    host_contract: EthContract,
    handler_contract: EthContract,
    monitor_handle: std::cell::RefCell<Option<tokio::task::JoinHandle<()>>>,
}
impl CeloChain {
    fn options_for_query(&self) -> web3::contract::Options {
        web3::contract::Options {
            gas: self.config.max_gas.map(web3::types::U256::from),
            gas_price: Some(web3::types::U256::from(self.config.gas_price)),
            ..Default::default()
        }
    }
    fn sender(&self) -> Result<web3::types::Address, Error> {
        let key_entry = self
            .keybase
            .get_key(&self.config.key_name)
            .map_err(Error::key_base)?;
        let addr = web3::types::Address::from_slice(&key_entry.address);
        Ok(addr)
    }
    fn query_state<State, Params>(
        &self,
        method: &str,
        height: ICSHeight,
        p: Params,
    ) -> Result<State, Error>
    where
        State: Message + std::any::Any + Default,
        Params: web3::contract::tokens::Tokenize + core::fmt::Debug,
    {
        if height.revision_number != self.config.id.version() {
            return Err(Error::celo_custom(String::from(
                "can't deal with different revision numbers",
            )));
        }
        let full_call = format!("IBCHost::{}({:?})", method, p);
        let block = web3::types::BlockId::Number(web3::types::BlockNumber::Number(
            web3::types::U64::from(height.revision_height),
        ));
        let (state_bytes, present): (Vec<u8>, bool) = self
            .rt
            .block_on(self.host_contract.query(
                method,
                p,
                self.sender()?,
                self.options_for_query(),
                Some(block),
            ))
            .map_err(|e| Error::web3_contract(full_call.clone(), e))?;
        if !present {
            return Err(Error::celo_custom(
                format!("{}:state not found", full_call,),
            ));
        }
        let state = State::decode(bytes::Bytes::from(state_bytes))
            .map_err(|e| Error::protobuf_decode(std::any::type_name::<State>().to_string(), e))?;
        Ok(state)
    }
}

impl MinimalChainEndpoint for CeloChain {
    fn id(&self) -> &ChainId {
        println!("giulio - CeloChain::id");
        &self.config.id
    }

    fn health_check(&self) -> Result<HealthCheck, Error> {
        self.rt
            .block_on(self.celo.web3().client_version())
            .map_err(|e| Error::web3(self.config.rpc_addr.to_string(), e))?;
        //TODO! check version compatibility
        //TODO! call to https://docs.rs/web3/0.17.0/web3/api/struct.Eth.html#method.syncing
        Ok(HealthCheck::Healthy)
    }

    fn keybase(&self) -> &KeyRing {
        println!("giulio - CeloChain::keybase");
        &self.keybase
    }

    fn keybase_mut(&mut self) -> &mut KeyRing {
        println!("giulio - CeloChain::keybase_mut");
        &mut self.keybase
    }

    fn get_signer(&mut self) -> Result<Signer, Error> {
        println!("giulio - CeloChain::get_signer");
        let s = Signer::from(String::from("this is a celo Signer"));
        Ok(s)
    }

    fn get_key(&mut self) -> Result<KeyEntry, Error> {
        self.keybase
            .get_key(&self.config.key_name)
            .map_err(Error::key_base)
    }

    fn send_messages_and_wait_commit(
        &mut self,
        proto_msgs: Vec<Any>,
    ) -> Result<Vec<IbcEvent>, Error> {
        let types: Vec<String> = proto_msgs.into_iter().map(|proto| proto.type_url).collect();
        todo!("giulio - CeloChain::send_messages_and_wait_commit - {:?}", types);
    }

    /// Queries
    fn query_latest_height(&self) -> Result<ICSHeight, Error> {
        let height = self
            .rt
            .block_on(self.celo.eth().block_number())
            .map_err(|e| Error::web3(self.config.rpc_addr.clone().into(), e))?;
        let h = ICSHeight {
            revision_number: self.config.id.version(),
            revision_height: height.as_u64(),
        };
        Ok(h)
    }

    fn query_client_state(
        &self,
        client_id: &ClientId,
        height: ICSHeight,
    ) -> Result<AnyClientState, Error> {
        let params = (client_id.to_string(),);
        let raw_tm_cl_state: ibc_proto::ibc::lightclients::tendermint::v1::ClientState =
            self.query_state("getClientState", height, params)?;
        let tm_state = ibc::ics07_tendermint::client_state::ClientState::try_from(raw_tm_cl_state)
            .map_err(Error::ics07)?;
        Ok(tm_state.wrap_any())
    }

    fn query_txs(&self, request: QueryTxRequest) -> Result<Vec<IbcEvent>, Error> {
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
        todo!(
            "giulio - CeloChain::query_client_connections - {:?}",
            request
        );
    }

    fn query_connection(
        &self,
        connection_id: &ConnectionId,
        height: ICSHeight,
    ) -> Result<ConnectionEnd, Error> {
        let params = (connection_id.to_string(),);
        let raw_state: ibc_proto::ibc::core::connection::v1::ConnectionEnd =
            self.query_state("getConnection", height, params)?;
        //let tm_state = ibc::ics07_tendermint::client_state::ClientState::try_from(raw_state)
        //.map_err(Error::ics07)?;

        todo!(
            "giulio - CeloChain::query_connection - {:?} - {:?}",
            connection_id,
            height
        );
    }

    fn query_connection_channels(
        &self,
        request: QueryConnectionChannelsRequest,
    ) -> Result<Vec<IdentifiedChannelEnd>, Error> {
        todo!()
    }

    fn query_clients(
        &self,
        request: QueryClientStatesRequest,
    ) -> Result<Vec<IdentifiedAnyClientState>, Error> {
        todo!("giulio - CeloChain::query_clients {:?}", request);
    }

    fn query_consensus_states(
        &self,
        request: QueryConsensusStatesRequest,
    ) -> Result<Vec<AnyConsensusStateWithHeight>, Error> {
        todo!("giulio - CeloChain::query_consensus_states {:?}", request);
    }

    fn query_consensus_state(
        &self,
        client_id: ClientId,
        consensus_height: ICSHeight,
        query_height: ICSHeight,
    ) -> Result<AnyConsensusState, Error> {
        if consensus_height.revision_number != query_height.revision_number {
            return Err(Error::celo_custom(String::from(
                "can't deal with different revision numbers",
            )));
        }
        let params = (
            client_id.to_string(),
            web3::types::U256::from(query_height.revision_height),
        );
        let raw_state: ibc_proto::ibc::lightclients::tendermint::v1::ConsensusState =
            self.query_state("getConsensusState", query_height, params)?;
        let tm_state = ibc::ics07_tendermint::consensus_state::ConsensusState::try_from(raw_state)
            .map_err(Error::ics07)?;
        Ok(tm_state.wrap_any())
    }

    fn query_channel(
        &self,
        port_id: &PortId,
        channel_id: &ChannelId,
        height: ICSHeight,
    ) -> Result<ChannelEnd, Error> {
        todo!()
    }
}

impl ChainEndpoint for CeloChain {
    type ChainConfig = CeloChainConfig;
    type LightBlock = CeloBlock;
    type Header = WasmHeader;
    type ConsensusState = WasmConsensusState;
    type ClientState = WasmClientState;
    type LightClient = CeloLightClient;

    fn bootstrap(config: CeloChainConfig, rt: Arc<TokioRuntime>) -> Result<Self, Error> {
        println!("giulio - CeloChain::bootstrap");
        //TODO! (check chainId from config and from rpc node match)
        let keybase = KeyRing::new(Store::Test, &config.account_prefix, &config.id)
            .map_err(Error::key_base)?;
        let celo_node_address = config.websocket_addr.to_string();
        let transport = rt
            .block_on(web3::transports::WebSocket::new(&celo_node_address))
            .map_err(|e| Error::web3(celo_node_address.clone(), e))?;
        let celo = web3::Web3::new(transport);
        let client_contract = build_contract(
            celo.eth(),
            &config.client_contract_abi_json,
            config.client_contract_address,
        )?;
        let host_contract = build_contract(
            celo.eth(),
            &config.host_contract_abi_json,
            config.host_contract_address,
        )?;
        let handler_contract = build_contract(
            celo.eth(),
            &config.handler_contract_abi_json,
            config.handler_contract_address,
        )?;
        let s = Self {
            config,
            rt,
            keybase,
            celo,
            client_contract,
            host_contract,
            handler_contract,
            monitor_handle: std::cell::RefCell::new(None),
        };
        Ok(s)
    }
    fn init_light_client(&self) -> Result<CeloLightClient, Error> {
        let lt = CeloLightClient {
            client: self.celo.clone(),
            rpc_addr: self.config.rpc_addr.clone(),
            chain_revision: self.config.id.version(),
            rt: self.rt.clone(),
        };
        Ok(lt)
    }
    fn init_event_monitor(
        &self,
        rt: Arc<TokioRuntime>,
    ) -> Result<(EventReceiver, TxMonitorCmd), Error> {
        println!("giulio - CeloChain::init_event_monitor");
        let (sender, receiver) = crossbeam_channel::unbounded::<EventResult<EventBatch>>();
        let (tx_sender, tx_receiver) = crossbeam_channel::unbounded::<MonitorCmd>();
        let mut subscription = rt
            .block_on(self.celo.eth_subscribe().subscribe_new_heads())
            .map_err(|e| Error::web3(self.config.websocket_addr.to_string(), e))?;
        let chain_id = self.config.id.clone();
        let revision_number = self.config.id.version();
        let f = async move {
            loop {
                match tx_receiver.try_recv() {
                    Ok(command) => match command {
                        MonitorCmd::Shutdown => return,
                    },
                    Err(crossbeam_channel::TryRecvError::Disconnected) => return,
                    Err(crossbeam_channel::TryRecvError::Empty) => {}
                }
                let event = match subscription.try_next().await {
                    Ok(Some(header)) => {
                        let height = Height {
                            revision_number,
                            revision_height: header
                                .number
                                .expect("blockheader with no number")
                                .as_u64(),
                        };
                        let ev = EventBatch {
                            chain_id: chain_id.clone(),
                            height,
                            events: vec![IbcEvent::NewBlock(NewBlock { height })],
                        };
                        Ok(ev)
                    }
                    Ok(None) => Err(EventError::collect_events_failed(String::from(
                        "header_subscription: empty event",
                    ))),
                    Err(err) => Err(EventError::web3_rpc(err)),
                };
                sender.send(event);
            }
        };
        self.monitor_handle.replace(Some(self.rt.spawn(f)));
        Ok((receiver, tx_sender))
    }

    fn shutdown(self) -> Result<(), Error> {
        if let Some(handle) = self.monitor_handle.take() {
            handle.abort();
        }
        Ok(())
    }

    fn send_messages_and_wait_check_tx(
        &mut self,
        proto_msgs: Vec<Any>,
    ) -> Result<Vec<TxResponse>, Error> {
        todo!("send_messages_and_wait_check_tx")
    }
    fn query_commitment_prefix(&self) -> Result<CommitmentPrefix, Error> {
        todo!("giulio - CeloChain::query_commitment_prefix");
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

    fn query_channels(
        &self,
        request: QueryChannelsRequest,
    ) -> Result<Vec<IdentifiedChannelEnd>, Error> {
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
        todo!("giulio - CeloChain::build_client_state");
        // TODO: these config params should all be loaded from the config
        let celo_cl = celo_types::client::LightClientState {
            epoch_size: self.config.epoch_size,
            allowed_clock_skew: self.config.allowed_clock_skew,
            trusting_period: self.config.trusting_period,
            upgrade_path: self.config.upgrade_path,
            verify_epoch_headers: self.config.verify_epoch_headers,
            verify_non_epoch_headers: self.config.verify_non_epoch_headers,
            verify_header_timestamp: self.config.verify_header_timestamp,
            allow_update_after_expiry: self.config.allow_update_after_expiry,
            allow_update_after_misbehavior: self.config.allow_update_after_misbehavior,
        };

        let cl = WasmClientState {
            chain_id: self.config.id.clone(),
            code_id: hex::decode(&self.config.code_id).map_err(Error::hex_decode)?,
            data: rlp::encode(&celo_cl).to_vec(),
            is_frozen: false,
            latest_height: height,
        };
        Ok(cl)
    }

    fn build_consensus_state(
        &self,
        block: Self::LightBlock,
    ) -> Result<Self::ConsensusState, Error> {
        build_wasm_consensus_state(block, &self.config.code_id)
    }

    fn build_header(
        &self,
        trusted: ICSHeight,
        target: ICSHeight,
        client_state: &AnyClientState,
        light_client: &mut Self::LightClient,
    ) -> Result<(Self::Header, Vec<Self::Header>), Error> {
        if trusted.revision_number != target.revision_number
            || trusted.revision_number != self.config.id.version()
        {
            return Err(Error::celo_custom(String::from(
                "can't deal with different revision numbers",
            )));
        }
        let supporting_heights: Vec<u64> =
            (trusted.revision_height + 1..target.revision_height).collect();
        let res = self
            .rt
            .block_on(light_client.get_headers(&supporting_heights, target.revision_height))?;
        let supportings = res
            .supporting
            .into_iter()
            .map(|h| build_wasm_header(h, trusted.revision_number))
            .collect();
        let target = build_wasm_header(res.target, trusted.revision_number);
        Ok((target, supportings))
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct TruffleAbi {
    abi: Vec<serde_json::Value>,
}

fn build_contract(
    eth: EthAPI,
    abi_fname: &std::path::Path,
    address: web3::types::Address,
) -> Result<EthContract, Error> {
    println!("giulio - build_contract {:?}", abi_fname);
    let abi_file = std::fs::File::open(abi_fname).map_err(Error::io)?;
    let reader = std::io::BufReader::new(abi_file);
    let abi: TruffleAbi = serde_json::from_reader(reader).map_err(Error::serde_json)?;
    let serialized_abi: Vec<u8> = serde_json::to_vec(&abi.abi).map_err(Error::serde_json)?;

    let contract = web3::contract::Contract::from_json(eth, address, &serialized_abi)
        .map_err(Error::eth_abi)?;
    Ok(contract)
}
