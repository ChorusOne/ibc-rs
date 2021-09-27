use crate::ics02_client::client_state;
use crate::ics02_client::client_type::ClientType;
use crate::ics24_host::identifier::ChainId;
use crate::ics28_wasm::error::Error;
use crate::Height;
use ibc_proto::ibc::lightclients::wasm::v1::ClientState as RawClientState;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use tendermint_proto::Protobuf;
use crate::ics23_commitment::specs::ProofSpecs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientState {
    pub data: Vec<u8>,
    pub code_id: Vec<u8>,
    pub chain_id: ChainId,
    pub latest_height: Height,
    pub is_frozen: bool,
    //pub proof: ProofSpecs,
}

impl client_state::ClientState for ClientState {
    fn chain_id(&self) -> ChainId {
        self.chain_id.clone()
    }
    fn client_type(&self) -> ClientType {
        ClientType::Wasm
    }
    fn latest_height(&self) -> Height {
        self.latest_height
    }
    fn is_frozen(&self) -> bool {
        self.is_frozen
    }
    fn wrap_any(self) -> client_state::AnyClientState {
        client_state::AnyClientState::Wasm(self)
    }
}

impl Protobuf<RawClientState> for ClientState {}

impl TryFrom<RawClientState> for ClientState {
    type Error = Error;
    fn try_from(raw: RawClientState) -> Result<Self, Self::Error> {
        let h = raw.latest_height.ok_or_else(|| {
            Error::missing_raw_client_state(String::from("missing latest_height"))
        })?;
        let s = Self {
            chain_id: ChainId::new(String::from("celo"), 1),
            latest_height: h.into(),
            is_frozen: false,
            data: raw.data,
            code_id: raw.code_id,
        };
        Ok(s)
    }
}

impl From<ClientState> for RawClientState {
    fn from(c: ClientState) -> Self {
        Self {
            data: c.data,
            code_id: c.code_id,
            latest_height: Some(c.latest_height.into()),
            proof_specs: ProofSpecs::cosmos().into(),
        }
    }
}
