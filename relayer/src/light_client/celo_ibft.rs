use crate::chain::celo::compatibility::*;
use crate::light_client::Verified;
use crate::{chain::CeloChain, error::Error};
use celo_types::istanbul::ValidatorData;
use celo_types::Header as CeloHeader;
use celo_types::{
    client::LightClientState as CeloClientState,
    consensus::LightConsensusState as CeloConsensusState,
};
use futures::future::try_join_all;
use ibc::ics02_client::client_state::AnyClientState;
use ibc::ics02_client::client_type::ClientType;
use ibc::ics02_client::events::UpdateClient;
use ibc::ics02_client::misbehaviour::MisbehaviourEvidence;
use ibc::ics28_wasm::header::Header as WasmHeader;
use ibc::{downcast, Height};
use std::convert::{identity, TryFrom};
use web3::types::{BlockId, BlockNumber, U64};

use std::sync::Arc;
use tokio::runtime::Runtime as TokioRuntime;

pub struct LightClient {
    pub client: Web3Client,
    pub rpc_addr: url::Url,
    pub chain_revision: u64,
    pub rt: Arc<TokioRuntime>,
}

impl super::LightClient<CeloChain> for LightClient {
    fn header_and_minimal_set(
        &mut self,
        trusted: Height,
        target: Height,
        client_state: &AnyClientState,
    ) -> Result<Verified<WasmHeader>, Error> {
        println!(
            "giulio LightClient::header_and_minimal_set - {:?} - {:?}",
            trusted, target
        );
        if trusted.revision_number != target.revision_number
            || trusted.revision_number != self.chain_revision
        {
            return Err(Error::celo_custom(String::from(
                "can't deal with different revision numbers",
            )));
        }
        let wasm_cl_state = downcast!(client_state => AnyClientState::Wasm).ok_or_else(|| {
            Error::client_type_mismatch(ClientType::Wasm, client_state.client_type())
        })?;
        let _celo_cls: CeloClientState =
            rlp::decode(&wasm_cl_state.data).map_err(|e| Error::rlp(e))?;
        //TODO! use clientstate to find out what Consensuses states we need
        let supporting_heights: Vec<u64> =
            (trusted.revision_height + 1..target.revision_height).collect();
        let headers = self
            .rt
            .block_on(self.get_headers(&supporting_heights, target.revision_height))?;
        let supporting_wasm_headers: Vec<WasmHeader> = headers
            .supporting
            .into_iter()
            .map(|celo_header| {
                let h = Height {
                    revision_number: trusted.revision_number,
                    revision_height: celo_header.number.as_u64(),
                };
                WasmHeader {
                    height: h,
                    data: rlp::encode(&celo_header).to_vec(),
                }
            })
            .collect();
        let target_wasm_header = WasmHeader {
            height: Height {
                revision_number: target.revision_number,
                revision_height: headers.target.number.as_u64(),
            },
            data: rlp::encode(&headers.target).to_vec(),
        };
        let v = Verified {
            target: target_wasm_header,
            supporting: supporting_wasm_headers,
        };
        Ok(v)
    }

    fn verify(
        &mut self,
        trusted: Height,
        target: Height,
        client_state: &AnyClientState,
    ) -> Result<Verified<CeloBlock>, Error> {
        println!(
            "giulio CeloLightClient::verify {:?} -> {:?}",
            trusted, target
        );
        if trusted.revision_number != target.revision_number
            || trusted.revision_number != self.chain_revision
        {
            return Err(Error::celo_custom(String::from(
                "can't deal with different revision numbers",
            )));
        }
        let wasm_cl_state = downcast!(client_state => AnyClientState::Wasm).ok_or_else(|| {
            Error::client_type_mismatch(ClientType::Wasm, client_state.client_type())
        })?;
        let _celo_cls: CeloClientState =
            rlp::decode(&wasm_cl_state.data).map_err(|e| Error::rlp(e))?;
        //TODO! use clientstate to find out what Consensuses states we need
        let supporting_heights: Vec<u64> =
            (trusted.revision_height + 1..target.revision_height).collect();
        let headers = self
            .rt
            .block_on(self.get_headers(&supporting_heights, target.revision_height))?;
        let consensuses = self
            .rt
            .block_on(self.get_consensuses(&supporting_heights, target.revision_height))?;
        let supporting_blocks = headers
            .supporting
            .into_iter()
            .zip(consensuses.supporting.into_iter())
            .map(|(header, consensus_state)| CeloBlock {
                header,
                consensus_state,
            })
            .collect();
        let target_block = CeloBlock {
            header: headers.target,
            consensus_state: consensuses.target,
        };
        let v = Verified {
            supporting: supporting_blocks,
            target: target_block,
        };
        Ok(v)
    }
    fn check_misbehaviour(
        &mut self,
        _update: UpdateClient,
        _client_state: &AnyClientState,
    ) -> Result<Option<MisbehaviourEvidence>, Error> {
        todo!()
    }
    fn fetch(&mut self, height: Height) -> Result<CeloBlock, Error> {
        let header = self
            .rt
            .block_on(self.get_headers(&[], height.revision_height))?;
        let consensus_state = self
            .rt
            .block_on(self.get_consensuses(&[], height.revision_height))?;
        let block = CeloBlock {
            header: header.target,
            consensus_state: consensus_state.target,
        };
        Ok(block)
    }
}

impl LightClient {
    pub async fn get_headers(
        &self,
        supporting: &[u64],
        target: u64,
    ) -> Result<Verified<CeloHeader>, Error> {
        let mut support_calls: Vec<_> = Vec::new();
        for number in supporting {
            let block_id = BlockId::Number(BlockNumber::Number(U64::from(*number)));
            let call = self.client.eth().block(block_id);
            support_calls.push(call);
        }
        let block_id = BlockId::Number(BlockNumber::Number(U64::from(target)));
        let target_call = self.client.eth().block(block_id);

        let support_blocks = try_join_all(support_calls)
            .await
            .map_err(|e| Error::web3(self.rpc_addr.to_string(), e))?;
        let support_headers: Vec<CeloHeader> = support_blocks
            .into_iter()
            .filter_map(identity)
            .map(CeloHeader::try_from)
            .collect::<Result<_, _>>()
            .map_err(|_| Error::celo_custom(String::from("Block -> Header conversion fail")))?;

        let target_block = target_call
            .await
            .map_err(|e| Error::web3(self.rpc_addr.to_string(), e))?
            .ok_or_else(|| Error::celo_custom(String::from("target header not found")))?;
        let target = CeloHeader::try_from(target_block)
            .map_err(|_| Error::celo_custom(String::from("Block -> Header conversion fail")))?;

        let v = Verified {
            supporting: support_headers,
            target,
        };
        Ok(v)
    }

    async fn get_consensuses(
        &self,
        supporting: &[u64],
        target: u64,
    ) -> Result<Verified<CeloConsensusState>, Error> {
        let mut support_calls: Vec<_> = Vec::new();
        for number in supporting {
            let block_id = BlockId::Number(BlockNumber::Number(U64::from(*number)));
            let call = self.client.istanbul().snapshot::<ValidatorData>(block_id);
            support_calls.push(call);
        }
        let block_id = BlockId::Number(BlockNumber::Number(U64::from(target)));
        let target_call = self.client.istanbul().snapshot::<ValidatorData>(block_id);

        let support_snapshots = try_join_all(support_calls)
            .await
            .map_err(|e| Error::web3(self.rpc_addr.to_string(), e))?;
        let support_consensuses: Vec<CeloConsensusState> = support_snapshots
            .into_iter()
            .filter_map(identity)
            .map(CeloConsensusState::from)
            .collect();

        let target_snapshot = target_call
            .await
            .map_err(|e| Error::web3(self.rpc_addr.to_string(), e))?
            .ok_or_else(|| Error::celo_custom(String::from("target snapshot not found")))?;
        let target = CeloConsensusState::from(target_snapshot);

        let v = Verified {
            supporting: support_consensuses,
            target,
        };
        Ok(v)
    }
}
