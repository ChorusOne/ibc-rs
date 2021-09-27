use serde::{Deserialize, Serialize};

use celo_light_client::{
    contract::types::state::LightClientState as RawClientState,
    contract::types::state::LightConsensusState as RawConsensusState, Header as RawHeader,
};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SimulationBlock {
    pub header: RawHeader,
    pub initial_consensus_state: RawConsensusState,
    pub initial_client_state: RawClientState,
}

pub mod tm {
    use ibc::ics02_client::client_consensus::AnyConsensusStateWithHeight;
    use ibc::ics02_client::client_consensus::{
        ConsensusState, TENDERMINT_CONSENSUS_STATE_TYPE_URL,
    };
    use ibc::ics02_client::client_state::{ClientState, TENDERMINT_CLIENT_STATE_TYPE_URL};
    use ibc::ics02_client::client_type::ClientType;
    use ibc::ics02_client::header::TENDERMINT_HEADER_TYPE_URL;
    use ibc::ics02_client::height::Height;
    use ibc::ics02_client::trust_threshold::TrustThreshold;
    use ibc::ics03_connection::connection::{ConnectionEnd, Counterparty, State as ConnState};
    use ibc::ics03_connection::msgs::conn_open_try::MsgConnectionOpenTry;
    use ibc::ics07_tendermint::client_state::AllowUpdate;
    use ibc::ics07_tendermint::client_state::ClientState as TMClientState;
    use ibc::ics07_tendermint::consensus_state::ConsensusState as TMConsensusState;
    use ibc::ics07_tendermint::header::Header as TMHeader;
    use ibc::ics23_commitment::commitment::{CommitmentPrefix, CommitmentRoot};
    use ibc::ics24_host::identifier::{ChainId, ClientId, ConnectionId};
    use ibc_proto::ibc::core::client::v1::{MsgCreateClient, MsgUpdateClient};
    use ibc_proto::ibc::core::connection::v1::MsgConnectionOpenTry as RawMsgConnectionOpenTry;
    use ibc_proto::ibc::lightclients::tendermint::v1::{
        ClientState as RawTMClientState, ConsensusState as RawTMConsensusState,
        Header as RawTMHeader,
    };
    use tendermint::{Hash, Time};

    use prost::Message;
    use std::convert::TryFrom;
    use std::str::FromStr;
    use std::time::Duration;

    pub struct DummyTendermint {
        pub cls: Vec<TMClientState>,
        pub css: Vec<TMConsensusState>,
        pub conns: Vec<ConnectionEnd>,
    }
    impl DummyTendermint {
        pub fn from_create_client_msg(crc_msg: MsgCreateClient) -> Self {
            let clstate_msg = crc_msg.client_state.unwrap_or_default();
            if clstate_msg.type_url != TENDERMINT_CLIENT_STATE_TYPE_URL {
                panic!("not a tendermint client state");
            };
            let raw_cl = RawTMClientState::decode(clstate_msg.value.as_slice()).unwrap_or_default();
            let csstate_msg = crc_msg.consensus_state.unwrap_or_default();
            if csstate_msg.type_url != TENDERMINT_CONSENSUS_STATE_TYPE_URL {
                panic!("not a tendermint consensus state");
            };
            let raw_cs =
                RawTMConsensusState::decode(csstate_msg.value.as_slice()).unwrap_or_default();

            Self {
                cls: vec![TMClientState::try_from(raw_cl).unwrap()],
                css: vec![TMConsensusState::try_from(raw_cs).unwrap()],
                conns: Vec::new(),
            }
        }
        pub fn client_id(&self) -> ClientId {
            ClientId::new(ClientType::Tendermint, 1).unwrap()
        }
        pub fn height(&self) -> Height {
            self.cls.last().unwrap().latest_height.clone()
        }
        pub fn client_state(&self, id :&ClientId, h: Height) -> Option<TMClientState> {
            if *id != self.client_id(){
                return None;
            }
            self.cls.iter().find(|c| c.latest_height == h).cloned()
        }
        pub fn consensus_states_with_height(&self) -> Vec<AnyConsensusStateWithHeight> {
            self.css
                .iter()
                .enumerate()
                .map(|(idx, cs)| AnyConsensusStateWithHeight {
                    height: self.cls.get(idx).unwrap().latest_height.clone(),
                    consensus_state: cs.clone().wrap_any(),
                })
            .collect()
        }
        pub fn update(&mut self, msg: MsgUpdateClient) {
            let head = msg.header.unwrap();
            if head.type_url != TENDERMINT_HEADER_TYPE_URL {
                panic!("not a tendermint header");
            };
            let rawhead = RawTMHeader::decode(head.value.as_slice()).unwrap();
            let tmhead = TMHeader::try_from(rawhead).unwrap();
            self.cls
                .push(self.cls.last().unwrap().clone().with_header(tmhead.clone()));
            self.css.push(TMConsensusState::from(tmhead));
        }
        pub fn conn_try_open(
            &mut self,
            raw_msg: RawMsgConnectionOpenTry,
            ) -> (ConnectionId, ConnectionEnd) {
            let msg = MsgConnectionOpenTry::try_from(raw_msg).unwrap();
            println!(
                "giulio - DummyTendermint::conn_try_open - {:?} - {:?}",
                msg.client_id, msg.counterparty().client_id(),
                );
            let cend = ConnectionEnd::new(
                ConnState::TryOpen,
                self.client_id(),
                msg.counterparty,
                msg.counterparty_versions,
                msg.delay_period,
                );
            self.conns.push(cend.clone());
            (ConnectionId::new((self.conns.len() - 1) as u64), cend)
        }
        pub fn conn_end(&self, conn_id: &ConnectionId) -> Option<ConnectionEnd> {
            self.conns
                .iter()
                .enumerate()
                .find(|(idx, c)| ConnectionId::new(*idx as u64) == *conn_id)
                .map(|(_, c)| c)
                .cloned()
        }
    }

    impl Default for DummyTendermint {
        fn default() -> Self {
            Self {
                cls: Vec::new(),
                css: Vec::new(),
                conns: Vec::new(),
            }
        }
    }
}
