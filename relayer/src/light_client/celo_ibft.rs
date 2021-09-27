#![allow(unused)]

use celo_light_client::ToRlp;
use std::convert::TryInto;
use crate::chain::celo::compatibility::SimulationBlock;
use crate::light_client::Verified;
use crate::{chain::CeloChain, error::Error};
use celo_light_client::{
    contract::types::state::LightClientState as RawClientState,
    contract::types::wasm::ConsensusState as RawConsensusState, Header as RawHeader,
};
use ibc::ics02_client::client_state::AnyClientState;
use ibc::ics02_client::events::UpdateClient;
use ibc::ics02_client::misbehaviour::MisbehaviourEvidence;
use ibc::ics28_wasm::header::Header as WasmHeader;
use ibc::{downcast, Height};

pub struct LightClient {
    pub blocks: Vec<SimulationBlock>,
}

impl super::LightClient<CeloChain> for LightClient {
    fn header_and_minimal_set(
        &mut self,
        trusted: Height,
        target: Height,
        client_state: &AnyClientState,
        ) -> Result<Verified<WasmHeader>, Error> {
        println!("giulio LightClientStateCeloIbft::header_and_minimal_set - {:?} - {:?}", trusted, target);
        let Verified{target, supporting} = self.verify (trusted, target, client_state).unwrap();

        let target_h = block_to_header(target);
        let supporting_h : Vec<WasmHeader> = supporting.into_iter().map(block_to_header).collect();

        let verified = Verified {
            target: target_h,
            supporting: supporting_h,
        };
        Ok(verified)
    }
    fn verify(
        &mut self,
        trusted: Height,
        target: Height,
        client_state: &AnyClientState,
        ) -> Result<Verified<SimulationBlock>, Error> {
        println!("giulio CeloLightClient::verify {:?} -> {:?}", trusted, target);
        if target.revision_height == 0 {
            let v = Verified{
                supporting: Vec::new(),
                target : self.blocks.first().unwrap().clone (),
            };
            return Ok(v);
        }
        let first_block = self.blocks.first().unwrap();
        let b = self
            .blocks
            .iter()
            .find(|b| {
                let height : u64 = b.header.number.clone().try_into().unwrap();
                height == target.revision_height
            })
        .expect(&format!("height {} not found", target.revision_height ))
            .clone();
        let bs: Vec<SimulationBlock> = self
            .blocks
            .iter()
            .filter(|b| {
                b.initial_consensus_state.number > trusted.revision_height
                    && b.initial_consensus_state.number < target.revision_height
            })
        .cloned()
            .collect();
        let v = Verified {
            supporting: bs,
            target: b,
        };
        Ok(v)
    }
    fn check_misbehaviour(
        &mut self,
        update: UpdateClient,
        client_state: &AnyClientState,
        ) -> Result<Option<MisbehaviourEvidence>, Error> {
        todo!()
    }
    fn fetch(&mut self, height: Height) -> Result<SimulationBlock, Error> {
        todo!("giulio CeloLightClient::verify {:?}", height);
    }
}


fn block_to_header(block : SimulationBlock) -> WasmHeader {
    let h = Height {
        revision_number: 0,
        revision_height: block.header.number.clone().try_into().unwrap(),
    };
    let wasm_h = WasmHeader {
        height: h,
        data: block.header.to_rlp(), // rlp::encode(&block.header).as_ref().to_vec(),
    };
    wasm_h
}
