use anyhow::Result;
use near_units::parse_near;
use serde_json::json;
use std::{env, fs};
use workspaces::{Account, Contract};

mod test_utils;
use crate::test_utils::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let wasm_amm_arg: &str = &(env::args().nth(1).unwrap());
    let wasm_amm_filepath = fs::canonicalize(env::current_dir()?.join(wasm_amm_arg))?;

    let worker = workspaces::sandbox().await?;

    // deploy amm
    let wasm_amm = std::fs::read(wasm_amm_filepath)?;
    let amm_contract = worker.dev_deploy(&wasm_amm).await?;

    // deploy and initialize test tokens
    let wasm_token_arg: &str = &(env::args().nth(2).unwrap());
    let wasm_token_filepath = fs::canonicalize(env::current_dir()?.join(wasm_token_arg))?;
    let wasm_token = std::fs::read(wasm_token_filepath)?;

    let token_contract_a = worker.dev_deploy(&wasm_token).await?;
    let token_a_init_result = token_contract_a
        .call("new")
        .args_json(json!({"name": "Token A", "decimals": 8}))
        .max_gas()
        .transact()
        .await?;
    assert!(token_a_init_result.is_success());

    let token_contract_b = worker.dev_deploy(&wasm_token).await?;
    let token_b_init_result = token_contract_b
        .call("new")
        .args_json(json!({"name": "Token B", "decimals": 16}))
        .max_gas()
        .transact()
        .await?;
    assert!(token_b_init_result.is_success());

    let token_contract_c = worker.dev_deploy(&wasm_token).await?;
    let token_c_init_result = token_contract_c
        .call("new")
        .args_json(json!({"name": "Token C", "decimals": 8}))
        .max_gas()
        .transact()
        .await?;
    assert!(token_c_init_result.is_success());

    println!(
        "AMM: {}\nToken A: {}\nToken B: {}",
        amm_contract.id(),
        token_contract_a.id(),
        token_contract_b.id()
    );

    // create user accounts
    let account = worker.dev_create_account().await?;

    let owner = account
        .create_subaccount("owner")
        .initial_balance(parse_near!("30 N"))
        .transact()
        .await?
        .into_result()?;

    let alice = account
        .create_subaccount("alice")
        .initial_balance(parse_near!("30 N"))
        .transact()
        .await?
        .into_result()?;

    let bob = account
        .create_subaccount("bob")
        .initial_balance(parse_near!("30 N"))
        .transact()
        .await?
        .into_result()?;

    // Register the AMM in Token A and Token B
    register_with_token(&owner, &amm_contract.id(), &token_contract_a).await?;
    register_with_token(&owner, &amm_contract.id(), &token_contract_b).await?;

    // begin tests
    test_init(&amm_contract, &owner, &token_contract_a, &token_contract_b).await?;
    test_ratio_is_zero_after_init(&amm_contract, &alice).await?;
    test_owner_deposit_modifies_ratio(&amm_contract, &token_contract_a, &token_contract_b, &owner)
        .await?;
    test_ft_transfer_does_not_change_balance(&amm_contract, &token_contract_a, &owner).await?;
    test_failed_swap_returns_tokens(&amm_contract, &token_contract_a, &token_contract_b, &bob).await?;
    test_swap(&amm_contract, &token_contract_a, &token_contract_b, &alice).await?;
    test_swap_with_foreign_token_fails(&amm_contract, &token_contract_a, &token_contract_b, &token_contract_c, &alice).await?;
    Ok(())
}

async fn test_init(
    amm_contract: &Contract,
    owner: &Account,
    token_a: &Contract,
    token_b: &Contract,
) -> Result<()> {
    let call_result = owner
        .call(amm_contract.id(), "new")
        .args_json(json!({"owner": owner.id(), "token_a": token_a.id(), "token_b": token_b.id()}))
        .max_gas()
        .transact()
        .await?;

    if call_result.is_failure() {
        println!("      Failed 🚫 test_init - initialization call failed");
    } else {
        println!("      Passed ✅ test_init");
    }

    Ok(())
}

async fn test_ratio_is_zero_after_init(amm_contract: &Contract, alice: &Account) -> Result<()> {
    if check_ratio_value(amm_contract, alice, 0).await? {
        println!("      Passed ✅ test_ratio_is_zero_after_init");
    } else {
        println!("      Failed 🚫 test_ratio_is_zero_after_init - ratio is not 0");
    }

    Ok(())
}

async fn test_owner_deposit_modifies_ratio(
    amm_contract: &Contract,
    token_a: &Contract,
    token_b: &Contract,
    owner: &Account,
) -> Result<()> {
    // mint tokens for owner
    mint_tokens(owner, token_a, "1000000000000".to_string()).await?;
    mint_tokens(owner, token_b, "1000000000000000000".to_string()).await?;

    // Deposit tokens in the AMM.
    transfer_tokens_to_amm(owner, token_a, amm_contract, "1000000000".to_string()).await?;
    assert!(
        check_amm_balance_value(amm_contract, owner, 1000000000, token_a).await?,
        "Balance for {} has not been updated accordingly in {}.",
        token_a.id(),
        amm_contract.id()
    );

    transfer_tokens_to_amm(
        owner,
        token_b,
        amm_contract,
        "1000000000000000000".to_string(),
    )
    .await?;
    assert!(
        check_amm_balance_value(amm_contract, owner, 1000000000000000000, token_b).await?,
        "Balance for {} has not been updated accordingly in {}.",
        token_a.id(),
        amm_contract.id()
    );

    // Ratio must be 1000
    if !check_ratio_value(amm_contract, owner, 1000).await? {
        println!("      Failed 🚫 test_owner_deposit_modifies_ratio - wrong value for ratio");
    }

    // Deposit again and see updated ratio
    transfer_tokens_to_amm(owner, token_a, amm_contract, "1000000000".to_string()).await?;

    assert!(
        check_amm_balance_value(amm_contract, owner, 2000000000, token_a).await?,
        "Balance for {} has not been updated accordingly in {}.",
        token_a.id(),
        amm_contract.id()
    );

    if check_ratio_value(amm_contract, owner, 2000).await? {
        println!("      Passed ✅ test_owner_deposit_modifies_ratio");
    } else {
        println!("      Failed 🚫 test_owner_deposit_modifies_ratio - wrong value for ratio after a new deposit");
    }

    Ok(())
}

async fn test_ft_transfer_does_not_change_balance(
    amm_contract: &Contract,
    token: &Contract,
    owner: &Account,
) -> Result<()> {
    let deposit = owner
        .call(token.id(), "ft_transfer")
        .args_json(json!({"receiver_id": amm_contract.id(), "amount": "1000000000"}))
        .deposit(1)
        .max_gas()
        .transact()
        .await?;
    assert!(
        deposit.is_success(),
        "Failed to deposit Token {} into {}.",
        token.id(),
        amm_contract.id()
    );

    if !check_amm_balance_value(amm_contract, owner, 2000000000, token).await? {
        println!("      Failed 🚫 test_ft_transfer_does_not_change_balance - ft_transfer updated balance");
    } else {
        println!("      Passed ✅ test_ft_transfer_does_not_change_balance");
    }

    Ok(())
}

async fn test_failed_swap_returns_tokens(
    amm_contract: &Contract,
    token_a: &Contract,
    token_b: &Contract,
    bob: &Account,
) -> Result<()> {
    // Bob will not be registered with token B so the swap should fail and Bob's A tokens must be returned.

    // mint tokens A for Bob. Minting function also registers Bob with token A.
    mint_tokens(bob, token_a, "100000000000".to_string()).await?;

    // Deposit tokens in the AMM.
    transfer_tokens_to_amm(bob, token_a, amm_contract, "1000000000".to_string()).await?;

    if check_user_balance_value(token_a, bob, 100000000000).await?
        && check_amm_balance_value(amm_contract, bob, 2000000000, token_a).await?
        && check_amm_balance_value(amm_contract, bob, 1000000000000000000, token_b).await?
    {
        println!("      Passed ✅ test_failed_swap_returns_tokens");
    } else {
        println!(
            "      Failed 🚫 test_failed_swap_returns_tokens - balances should have not changed"
        );
    }

    Ok(())
}

async fn test_swap(
    amm_contract: &Contract,
    token_a: &Contract,
    token_b: &Contract,
    alice: &Account,
) -> Result<()> {
    // mint tokens A for Alice. Minting function also registers Alice with token A.
    mint_tokens(alice, token_a, "100000000000".to_string()).await?;

    register_with_token(alice, alice.id(), token_b).await?;

    // Deposit tokens in the AMM.
    transfer_tokens_to_amm(alice, token_a, amm_contract, "1000000000".to_string()).await?;

    if check_user_balance_value(token_a, alice, 99000000000).await?
        && check_user_balance_value(token_b, alice, 333333333333333333).await?
        && check_amm_balance_value(amm_contract, alice, 3000000000, token_a).await?
        && check_amm_balance_value(amm_contract, alice, 666666666666666667, token_b).await?
    {
        println!("      Passed ✅ test_swap");
    } else {
        println!("      Failed 🚫 test_swap - miscalculation in token balances");
    }

    Ok(())
}

async fn test_swap_with_foreign_token_fails(
    amm_contract: &Contract,
    token_a: &Contract,
    token_b: &Contract,
    token_c: &Contract,
    alice: &Account,
) -> Result<()> {
    // mint tokens A for Alice. Minting function also registers Alice with token A.
    mint_tokens(alice, token_c, "100000000000".to_string()).await?;

    register_with_token(alice, alice.id(), token_b).await?;

    // malicious user registers the AMM with a foreign contract
    register_with_token(alice, amm_contract.id(), token_c).await?;

    // Deposit tokens in the AMM.
    transfer_tokens_to_amm(alice, token_c, amm_contract, "1000000000".to_string()).await?;

    if check_user_balance_value(token_c, alice, 100000000000).await?
        && check_user_balance_value(token_a, alice, 99000000000).await?
        && check_user_balance_value(token_b, alice, 333333333333333333).await?
        && check_amm_balance_value(amm_contract, alice, 3000000000, token_a).await?
        && check_amm_balance_value(amm_contract, alice, 666666666666666667, token_b).await?
    {
        println!("      Passed ✅ test_swap_with_foreign_token_fails");
    } else {
        println!("      Failed 🚫 test_swap_with_foreign_token_fails - balances have changed");
    }

    Ok(())
}
