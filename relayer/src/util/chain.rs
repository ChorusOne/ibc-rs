use alloc::sync::Arc;
use tokio::runtime::Runtime as TokioRuntime;

use crate::chain::handle::ChainHandle;
use crate::chain::runtime::ChainRuntime;
use crate::chain::{CeloChain, ChainEndpoint, CosmosSdkChain, MinimalChainEndpoint};
use crate::config::ChainConfig;
use crate::error::Error;

pub fn chainrutime_spawn<Chain: ChainHandle>(
    config: ChainConfig,
    rt: Arc<TokioRuntime>,
) -> Result<Chain, Error> {
    match config {
        ChainConfig::Tendermint(tm_config) => ChainRuntime::<CosmosSdkChain>::spawn(tm_config, rt),
        ChainConfig::Celo(cl_config) => ChainRuntime::<CeloChain>::spawn(cl_config, rt),
    }
}

pub fn bootstrap_chain(
    config: ChainConfig,
    rt: Arc<TokioRuntime>,
) -> Result<Box<dyn MinimalChainEndpoint>, Error> {
    match config {
        ChainConfig::Tendermint(tm_config) => {
            CosmosSdkChain::bootstrap(tm_config, rt).map(|cosmos| {
                let r: Box<dyn MinimalChainEndpoint> = Box::new(cosmos);
                r
            })
        }
        ChainConfig::Celo(cl_config) => CeloChain::bootstrap(cl_config, rt).map(|celo| {
            let r: Box<dyn MinimalChainEndpoint> = Box::new(celo);
            r
        }),
    }
}
