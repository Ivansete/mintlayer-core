use crate::chain::TxOutput;
use crate::primitives::Compact;
use parity_scale_codec::{Decode, Encode};

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum ConsensusData {
    #[codec(index = 0)]
    None,
    #[codec(index = 1)]
    PoW(PoWData),
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct PoWData {
    bits: Compact,
    nonce: u128,
    /// contains the block reward
    outputs: Vec<TxOutput>,
}

impl PoWData {
    pub fn new(bits: Compact, nonce: u128, outputs: Vec<TxOutput>) -> Self {
        PoWData {
            bits,
            nonce,
            outputs,
        }
    }
    pub fn bits(&self) -> Compact {
        self.bits
    }

    pub fn nonce(&self) -> u128 {
        self.nonce
    }

    pub fn outputs(&self) -> &[TxOutput] {
        &self.outputs
    }

    pub fn update_nonce(&mut self, nonce: u128) {
        self.nonce = nonce;
    }
}
