// Copyright (c) 2022 RBB S.r.l
// opensource@mintlayer.org
// SPDX-License-Identifier: MIT
// Licensed under the MIT License;
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://github.com/mintlayer/mintlayer-core/blob/master/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::BTreeMap;

use common::{
    chain::{
        tokens::{token_id, CoinOrTokenId, OutputValue, TokenData, TokensError},
        Transaction,
    },
    primitives::Amount,
};

use super::error::ConnectTransactionError;

pub fn insert_or_increase(
    total_amounts: &mut BTreeMap<CoinOrTokenId, Amount>,
    key: CoinOrTokenId,
    amount: Amount,
) -> Result<(), ConnectTransactionError> {
    match total_amounts.get_mut(&key) {
        Some(value) => {
            *value = (*value + amount).ok_or(ConnectTransactionError::TokensError(
                TokensError::CoinOrTokenOverflow,
            ))?;
        }
        None => {
            total_amounts.insert(key, amount);
        }
    }
    Ok(())
}

pub fn check_transferred_amount(
    inputs_total_map: &BTreeMap<CoinOrTokenId, Amount>,
    outputs_total_map: &BTreeMap<CoinOrTokenId, Amount>,
) -> Result<(), ConnectTransactionError> {
    for (coin_or_token_id, outputs_total) in outputs_total_map {
        // Is coin or token exist in inputs?
        let inputs_total = inputs_total_map
            .get(coin_or_token_id)
            .ok_or(ConnectTransactionError::MissingOutputOrSpent)?;

        // Does outputs exceed inputs?
        if outputs_total > inputs_total {
            return Err(ConnectTransactionError::AttemptToPrintMoney(
                *inputs_total,
                *outputs_total,
            ));
        }
    }
    Ok(())
}

// TODO(Anton): Probably this might be a better name
pub fn get_output_coin_or_tokenid(output_value: &OutputValue) -> Option<(CoinOrTokenId, &Amount)> {
    match output_value {
        OutputValue::Coin(amount) => Some((CoinOrTokenId::Coin, amount)),
        OutputValue::Token(token_data) => match token_data {
            TokenData::TokenTransferV1 { token_id, amount } => {
                Some((CoinOrTokenId::TokenId(*token_id), amount))
            }
            TokenData::TokenIssuanceV1 {
                token_ticker: _,
                amount_to_issue: _,
                number_of_decimals: _,
                metadata_uri: _,
            } => {
                // TODO: Might be it's not necessary at all?
                // if include_issuance {
                // ...
                // }
                None
            }
            TokenData::TokenBurnV1 {
                token_id,
                amount_to_burn,
            } => Some((CoinOrTokenId::TokenId(*token_id), amount_to_burn)),
        },
    }
}

// TODO(Anton): Probably this might be a better name
pub fn get_input_coin_or_tokenid(
    output_value: &OutputValue,
    tx: &Transaction,
) -> Result<(CoinOrTokenId, Amount), ConnectTransactionError> {
    Ok(match output_value {
        OutputValue::Coin(amount) => (CoinOrTokenId::Coin, *amount),
        OutputValue::Token(token_data) => match token_data {
            TokenData::TokenTransferV1 { token_id, amount } => {
                (CoinOrTokenId::TokenId(*token_id), *amount)
            }
            TokenData::TokenIssuanceV1 {
                token_ticker: _,
                amount_to_issue,
                number_of_decimals: _,
                metadata_uri: _,
            } => {
                let token_id = token_id(tx).ok_or(ConnectTransactionError::TokensError(
                    TokensError::TokenIdCantBeCalculated,
                ))?;
                (CoinOrTokenId::TokenId(token_id), *amount_to_issue)
            }
            TokenData::TokenBurnV1 {
                token_id: _,
                amount_to_burn: _,
            } => {
                /* Token have burned and can't be transferred */
                return Err(ConnectTransactionError::TokensError(
                    TokensError::AttemptToTransferBurnedTokens,
                ));
            }
        },
    })
}
