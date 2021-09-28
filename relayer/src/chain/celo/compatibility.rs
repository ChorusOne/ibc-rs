use serde::{Deserialize, Serialize};

use celo_types::{
    client::LightClientState as CeloClientState,
    consensus::LightConsensusState as CeloConsensusState, Header as CeloHeader,
};

use crate::error::Error;
use ibc::ics02_client::height::Height;
use ibc::ics28_wasm::{
    client_state::ClientState as WasmClientState,
    consensus_state::ConsensusState as WasmConsensusState, header::Header as WasmHeader,
};

pub type Web3Client = web3::Web3<web3::transports::WebSocket>;
pub type EthAPI = web3::api::Eth<web3::transports::WebSocket>;
pub type EthContract = web3::contract::Contract<web3::transports::WebSocket>;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct CeloBlock {
    pub header: CeloHeader,
    pub consensus_state: CeloConsensusState,
}

pub fn build_wasm_consensus_state(
    block: CeloBlock,
    code_id: &str,
) -> Result<WasmConsensusState, Error> {
    let cs = WasmConsensusState {
        data: rlp::encode(&block.consensus_state).to_vec(),
        code_id: hex::decode(code_id).map_err(Error::hex_decode)?,
        timestamp: chrono::NaiveDateTime::from_timestamp(block.header.time.as_u64() as i64, 0),
        root: ibc::ics23_commitment::commitment::CommitmentRoot::from_bytes(&[]),
    };
    Ok(cs)
}

pub fn build_wasm_header(header: CeloHeader, revision_number: u64) -> WasmHeader {
    let height = Height {
        revision_number,
        revision_height: header.number.as_u64(),
    };
    WasmHeader {
        data: rlp::encode(&header).to_vec(),
        height,
    }
}
