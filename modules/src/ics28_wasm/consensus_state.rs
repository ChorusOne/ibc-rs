use crate::ics02_client::client_consensus;
use crate::ics02_client::client_type::ClientType;
use crate::ics23_commitment::commitment::CommitmentRoot;
use crate::ics28_wasm::error::Error;
use chrono::NaiveDateTime;
use ibc_proto::ibc::core::commitment::v1::MerkleRoot;
use ibc_proto::ibc::lightclients::wasm::v1::ConsensusState as RawConsensusState;
use serde::Serialize;
use std::convert::Infallible;
use std::convert::TryFrom;
use tendermint_proto::Protobuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConsensusState {
    pub data: Vec<u8>,
    pub code_id: Vec<u8>,
    pub timestamp: NaiveDateTime,
    pub root: CommitmentRoot,
}

impl client_consensus::ConsensusState for ConsensusState {
    type Error = Infallible;

    fn client_type(&self) -> ClientType {
        ClientType::Wasm
    }
    fn root(&self) -> &CommitmentRoot {
        &self.root
    }
    fn validate_basic(&self) -> Result<(), Infallible> {
        unimplemented!()
    }
    fn wrap_any(self) -> client_consensus::AnyConsensusState {
        client_consensus::AnyConsensusState::Wasm(self)
    }
}

impl Protobuf<RawConsensusState> for ConsensusState {}

impl TryFrom<RawConsensusState> for ConsensusState {
    type Error = Error;
    fn try_from(raw: RawConsensusState) -> Result<Self, Self::Error> {
        let timestamp = NaiveDateTime::from_timestamp(raw.timestamp as i64, 0);
        let root: CommitmentRoot = raw
            .root
            .ok_or_else(|| Error::missing_raw_consensus_state(String::from("missing root")))?
            .hash
            .into();
        let s = Self {
            timestamp,
            root,
            data: raw.data,
            code_id: raw.code_id,
        };
        Ok(s)
    }
}

impl From<ConsensusState> for RawConsensusState {
    fn from(c: ConsensusState) -> Self {
        let root = MerkleRoot {
            hash: c.root.into_vec(),
        };
        Self {
            data: c.data,
            code_id: c.code_id,
            timestamp: c.timestamp.timestamp() as u64,
            root: Some(root),
        }
    }
}
