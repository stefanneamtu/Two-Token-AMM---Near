use anyhow::Result;
use near_sdk::json_types::U128;
use serde_json::json;
use workspaces::{Account, AccountId, Contract};

pub async fn register_with_token(
    caller: &Account,
    registree: &AccountId,
    token: &Contract,
) -> Result<()> {
    let register = caller
        .call(token.id(), "storage_deposit")
        .args_json(json!({"account_id": registree, "registration_only": true}))
        .deposit(10000000000000000000000)
        .max_gas()
        .transact()
        .await?;
    assert!(
        register.is_success(),
        "Failed to register {} for Token {}.",
        registree,
        token.id()
    );

    Ok(())
}

pub async fn check_ratio_value(
    amm_contract: &Contract,
    caller: &Account,
    expected_ratio: u128,
) -> Result<bool> {
    let call_result = caller
        .call(amm_contract.id(), "get_ratio")
        .args_json(json!({}))
        .max_gas()
        .transact()
        .await?;
    assert!(call_result.is_success(), "Failed to retrieve ratio.");

    let ratio: u128 = call_result
        .clone()
        .into_result()
        .unwrap()
        .json::<U128>()?
        .into();

    Ok(ratio == expected_ratio)
}

pub async fn check_amm_balance_value(
    amm_contract: &Contract,
    caller: &Account,
    expected_balance: u128,
    token: &Contract,
) -> Result<bool> {
    let call_result = caller
        .call(amm_contract.id(), "get_balance")
        .args_json(json!({ "token": token.id() }))
        .max_gas()
        .transact()
        .await?;
    assert!(call_result.is_success(), "Failed to retrieve AMM balance.");

    let balance: u128 = call_result
        .clone()
        .into_result()
        .unwrap()
        .json::<U128>()?
        .into();

    Ok(balance == expected_balance)
}
pub async fn check_user_balance_value(
    token: &Contract,
    caller: &Account,
    expected_balance: u128,
) -> Result<bool> {
    let call_result = caller
        .call(token.id(), "ft_balance_of")
        .args_json(json!({ "account_id": caller.id() }))
        .max_gas()
        .transact()
        .await?;
    assert!(call_result.is_success(), "Failed to retrieve user balance.");

    let balance: u128 = call_result
        .clone()
        .into_result()
        .unwrap()
        .json::<U128>()?
        .into();

    Ok(balance == expected_balance)
}

pub async fn mint_tokens(caller: &Account, token: &Contract, amount: String) -> Result<()> {
    // mint tokens for caller. Minting also registers the caller in the token
    let mint = caller
        .call(token.id(), "mint")
        .args_json(json!({"account_id": caller.id(), "amount": amount}))
        .max_gas()
        .transact()
        .await?;
    assert!(
        mint.is_success(),
        "{} failed to mint Token {}.",
        caller.id(),
        token.id()
    );

    Ok(())
}

pub async fn transfer_tokens_to_amm(
    caller: &Account,
    token: &Contract,
    amm_contract: &Contract,
    amount: String,
) -> Result<()> {
    let transfer = caller
        .call(token.id(), "ft_transfer_call")
        .args_json(json!({"receiver_id": amm_contract.id(), "amount": amount, "msg": ""}))
        .deposit(1)
        .max_gas()
        .transact()
        .await?;

    assert!(
        transfer.is_success(),
        "{} failed to deposit Token {} into the AMM.",
        caller.id(),
        token.id()
    );

    Ok(())
}
