use near_contract_standards::fungible_token::core::ext_ft_core::ext as ft_core_ext;
use near_contract_standards::fungible_token::metadata::ext_ft_metadata::ext as ft_metadata_ext;
use near_contract_standards::fungible_token::metadata::FungibleTokenMetadata;
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{
    env, log, near_bindgen, require, AccountId, Balance, Gas, PanicOnDefault, Promise,
    PromiseError, PromiseOrValue,
};
use uint::construct_uint;

const TGAS: Gas = Gas(10_000_000_000_000);

#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
struct Token {
    address: AccountId,
    balance: Balance,
    metadata: Option<TokenMetadata>,
}

impl Token {
    pub fn new(address: AccountId) -> Self {
        Self {
            address,
            balance: 0,
            metadata: None,
        }
    }
}

#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Clone, PanicOnDefault)]
pub struct TokenMetadata {
    name: String,
    symbol: String,
    decimals: u8,
}

impl TokenMetadata {
    pub fn new(name: String, symbol: String, decimals: u8) -> Self {
        Self {
            name,
            symbol,
            decimals,
        }
    }
}

// Create U256 to avoid overflows in swap calculations
construct_uint! {
    struct U256(4);
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct AMM {
    owner: AccountId,
    tokens: Vec<Token>,
}

#[near_bindgen]
impl AMM {
    #[init]
    pub fn new(owner: AccountId, token_a: AccountId, token_b: AccountId) -> Self {
        let amm = Self {
            owner,
            tokens: vec![Token::new(token_a.clone()), Token::new(token_b.clone())],
        };

        amm.update_metadata(token_a);
        amm.update_metadata(token_b);

        amm
    }

    pub fn update_metadata(&self, token: AccountId) -> Promise {
        // Ensure the metadata update request is for the right token.
        require!(
            token == self.tokens[0].address || token == self.tokens[1].address,
            "Wrong token provided."
        );

        let index = self.get_token_index(token.clone());
        let promise = ft_metadata_ext(token.clone())
            .with_static_gas(TGAS)
            .ft_metadata();

        promise.then(
            Self::ext(env::current_account_id())
                .with_static_gas(TGAS)
                .metadata_callback(index),
        )
    }

    #[private]
    pub fn metadata_callback(
        &mut self,
        #[callback_unwrap] call_result: FungibleTokenMetadata,
        index: usize,
    ) {
        self.tokens[index].metadata = Some(TokenMetadata::new(
            call_result.name,
            call_result.symbol,
            call_result.decimals,
        ));
    }

    pub fn get_metadata(&self, token: AccountId) -> TokenMetadata {
        let index = self.get_token_index(token.clone());
        require!(
            self.tokens[index].metadata.is_some(),
            "Metadata is not initialized!"
        );
        self.tokens[index].metadata.clone().unwrap()
    }

    pub fn get_balance(&self, token: AccountId) -> U128 {
        let index = self.get_token_index(token.clone());
        near_sdk::json_types::U128(self.tokens[index].balance)
    }

    pub fn get_ratio(&self) -> U128 {
        require!(
            self.tokens[0].metadata.is_some(),
            "Metadata not initialized for index 0."
        );
        require!(
            self.tokens[1].metadata.is_some(),
            "Metadata not initialized for index 1."
        );

        let balance_a: u128 = self.tokens[0].balance
            / 10_u128.pow(self.tokens[0].metadata.clone().unwrap().decimals.into());
        let balance_b: u128 = self.tokens[1].balance
            / 10_u128.pow(self.tokens[1].metadata.clone().unwrap().decimals.into());

        near_sdk::json_types::U128(balance_a * balance_b)
    }

    #[private]
    pub fn swap_callback(
        &mut self,
        balance_a: Balance,
        balance_b: Balance,
        amount: Balance,
        #[callback_result] call_result: Result<(), PromiseError>,
    ) -> PromiseOrValue<U128> {
        if call_result.is_err() {
            // Return the deposited tokens if the swap fails
            log!("Transfering the swapped tokens failed.");
            PromiseOrValue::Value(amount.into())
        } else {
            // Update the AMM balances
            self.tokens[0].balance = balance_a;
            self.tokens[1].balance = balance_b;

            PromiseOrValue::Value(0.into())
        }
    }
}

impl AMM {
    fn get_token_index(&self, token: AccountId) -> usize{
        if token == self.tokens[0].address {
            0
        } else {
            1
        }
    }

    fn owner_deposit(&mut self, token_in: usize, amount: Balance) {
        self.tokens[token_in].balance += amount;
    }

    fn swap(&mut self, sender_id: AccountId, token_in: usize, amount: Balance) -> Promise {
        let token_out = 1 - token_in;

        let new_balance_a = self.tokens[token_in].balance + amount;

        // Avoid multiplication overflow by using U256
        let token_out_amount = ((U256::from(self.tokens[token_out].balance) * U256::from(amount))
            / new_balance_a)
            .as_u128();

        require!(
            token_out_amount <= self.tokens[token_out].balance,
            "Not enough funds to complete the trade."
        );
        require!(token_out_amount > 0, "Cannot swap for 0 tokens.");

        let new_balance_b = self.tokens[token_out].balance - token_out_amount;

        ft_core_ext(self.tokens[token_out].address.clone())
            .with_static_gas(TGAS)
            .with_attached_deposit(1)
            .ft_transfer(sender_id, token_out_amount.into(), None)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(TGAS)
                    .swap_callback(new_balance_a, new_balance_b, amount),
            )
    }
}

#[near_bindgen]
impl FungibleTokenReceiver for AMM {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        drop(msg);
        let predecessor_id = env::predecessor_account_id();
        require!(
            predecessor_id == self.tokens[0].address || predecessor_id == self.tokens[1].address,
            "Token not supported."
        );

        let amount: Balance = amount.into();
        require!(amount > 0, "Amount must be positive.");

        let token_in: usize = self.get_token_index(predecessor_id);

        if sender_id == self.owner {
            self.owner_deposit(token_in, amount);
            PromiseOrValue::Value(near_sdk::json_types::U128(0))
        } else {
            self.swap(sender_id, token_in, amount).into()
        }
    }
}

/*

near call dev-1672856405734-99444394530569 new '{"owner": "ammdev.testnet", "token_a": "ft.predeployed.examples.testnet", "token_b": "ft.predeployed.examples.testnet"}' --account-id ammdev.testnet --gas=300000000000000

near call ft.predeployed.examples.testnet storage_deposit '{"account_id": "dev-1672856405734-99444394530569"}' --accountId ammdev.testnet --amount 0.00125

near call dev-1672856405734-99444394530569 get_metadata '{"ind": 0}' --account-id ammdev.testnet
near call dev-1672856405734-99444394530569 get_balance '{"ind": 0}' --account-id ammdev.testnet

near call ft.predeployed.examples.testnet ft_transfer '{"receiver_id": "'dev-1672856405734-99444394530569'", "amount": "1000"}' --accountId ammdev.testnet --depositYocto 1
near call ft.predeployed.examples.testnet ft_transfer_call '{"receiver_id": "'dev-1672856405734-99444394530569'", "amount": "1000", "msg": ""}' --accountId ammdev.testnet --depositYocto 1
near view ft.predeployed.examples.testnet ft_balance_of '{"account_id": "'dev-1672856405734-99444394530569'"}'  --account-id ammdev.testnet

near create-account bob.ammdev.testnet --masterAccount ammdev.testnet --initialBalance 10
near call ft.predeployed.examples.testnet storage_deposit '' --accountId bob.ammdev.testnet --amount 0.00125

near view ft.predeployed.examples.testnet ft_balance_of '{"account_id": "'ammdev.testnet'"}'  --account-id ammdev.testnet
near call ft.predeployed.examples.testnet ft_mint '{"account_id": "'ammdev.testnet'", "amount": "10000000000000000000000"}' --accountId ammdev.testnet

near call ft.predeployed.examples.testnet ft_transfer '{"receiver_id": "'bob.ammdev.testnet'", "amount": "19"}' --accountId ammdev.testnet --depositYocto 1

*/

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::{testing_env, VMContext};

    fn get_owner_ft_transfer_context(
        signer: AccountId,
        predecessor_account_id: AccountId,
        is_view: bool,
    ) -> VMContext {
        VMContextBuilder::new()
            .signer_account_id(signer)
            .predecessor_account_id(predecessor_account_id)
            .is_view(is_view)
            .build()
    }

    fn owner() -> AccountId {
        return "owner.testnet".to_string().parse().unwrap();
    }

    fn alice() -> AccountId {
        return "alice.testnet".to_string().parse().unwrap();
    }

    fn token_a() -> AccountId {
        return "token_a.testnet".to_string().parse().unwrap();
    }

    fn token_a_metadata() -> TokenMetadata {
        return TokenMetadata::new(
            "token_a".to_string().parse().unwrap(),
            "TA".to_string().parse().unwrap(),
            8,
        );
    }

    fn token_b() -> AccountId {
        return "token_b.testnet".to_string().parse().unwrap();
    }

    fn token_b_metadata() -> TokenMetadata {
        return TokenMetadata::new(
            "token_b".to_string().parse().unwrap(),
            "TB".to_string().parse().unwrap(),
            16,
        );
    }

    #[test]
    fn test_init() {
        let amm = AMM::new(owner(), token_a(), token_b());
        assert_eq!(amm.owner, owner());
        assert_eq!(amm.tokens[0].address, token_a());
        assert_eq!(amm.tokens[1].address, token_b());
    }

    #[test]
    #[should_panic]
    fn test_get_ratio_without_metadata() {
        let amm = AMM::new(owner(), token_a(), token_b());
        amm.get_ratio();
    }

    #[test]
    fn test_ratio() {
        let mut amm = AMM::new(owner(), token_a(), token_b());
        amm.tokens[0].metadata = Some(token_a_metadata());
        amm.tokens[0].balance = 1_000_000_000;
        amm.tokens[1].metadata = Some(token_b_metadata());
        amm.tokens[1].balance = 1_000_000_000_000_000_000;
        assert_eq!(amm.get_ratio(), near_sdk::json_types::U128(1000));
    }

    #[test]
    #[should_panic]
    fn test_swap_amount_zero() {
        let mut amm = AMM::new(owner(), token_a(), token_b());
        amm.swap(alice(), 0, 0);
    }

    #[test]
    #[should_panic]
    fn test_swap_for_zero_tokens() {
        let mut amm = AMM::new(owner(), token_a(), token_b());
        amm.tokens[0].metadata = Some(token_a_metadata());
        amm.tokens[0].balance = 1_000_000_000;
        amm.swap(alice(), 0, 10);
    }

    #[test]
    fn test_ft_on_transfer() {
        let mut amm = AMM::new(owner(), token_a(), token_b());

        // owner deposits token_a
        testing_env!(get_owner_ft_transfer_context(owner(), token_a(), false));
        amm.tokens[0].metadata = Some(token_a_metadata());
        amm.ft_on_transfer(
            owner(),
            near_sdk::json_types::U128(1_000_000_000),
            "".to_string(),
        );
        assert_eq!(
            amm.get_balance(token_a()),
            near_sdk::json_types::U128(1_000_000_000)
        );

        // owner deposits token_b
        testing_env!(get_owner_ft_transfer_context(owner(), token_b(), false));
        amm.tokens[1].metadata = Some(token_b_metadata());
        amm.ft_on_transfer(
            owner(),
            near_sdk::json_types::U128(1_000_000_000_000_000_000),
            "".to_string(),
        );
        assert_eq!(
            amm.get_balance(token_b()),
            near_sdk::json_types::U128(1_000_000_000_000_000_000)
        );

        // ratio gets updated accordingly
        assert_eq!(amm.get_ratio(), near_sdk::json_types::U128(1000));

        // user swaps tokens
        testing_env!(get_owner_ft_transfer_context(alice(), token_b(), false));
        amm.ft_on_transfer(
            alice(),
            near_sdk::json_types::U128(100_000_000_000_000_000),
            "".to_string(),
        );

        // ratio does not change after swap
        assert_eq!(amm.get_ratio(), near_sdk::json_types::U128(1000));

        // balances do not change after swap (we can't test balance updates with unit tests - see integration tests)
        // this happens because of the cross contract call
        assert_eq!(
            amm.get_balance(token_a()),
            near_sdk::json_types::U128(1_000_000_000)
        );
        assert_eq!(
            amm.get_balance(token_b()),
            near_sdk::json_types::U128(1_000_000_000_000_000_000)
        );

        // owner deposits more token_b
        testing_env!(get_owner_ft_transfer_context(owner(), token_b(), false));
        amm.ft_on_transfer(
            owner(),
            near_sdk::json_types::U128(1_000_000_000_000_000_000),
            "".to_string(),
        );
        assert_eq!(
            amm.get_balance(token_b()),
            near_sdk::json_types::U128(2_000_000_000_000_000_000)
        );

        // ratio gets updated accordingly
        assert_eq!(amm.get_ratio(), near_sdk::json_types::U128(2000));
    }

    #[test]
    fn test_overflow_in_swap_and_ratio() {
        let mut amm = AMM::new(owner(), token_a(), token_b());

        // Large u128 number to test for overflows
        const TEST_AMOUNT: U128 = near_sdk::json_types::U128(u128::MAX / 2);

        // pick higher decimals so that the ratio does not overflow and fits in U128
        let mut temp_metadata = token_a_metadata();
        temp_metadata.decimals = 20;
        amm.tokens[0].metadata = Some(temp_metadata);

        // owner deposits token_a
        testing_env!(get_owner_ft_transfer_context(owner(), token_a(), false));
        amm.ft_on_transfer(owner(), TEST_AMOUNT, "".to_string());
        assert_eq!(amm.get_balance(token_a()), TEST_AMOUNT);

        // pick higher decimals so that the ratio does not overflow and fits in U128
        let mut temp_metadata = token_b_metadata();
        temp_metadata.decimals = 18;
        amm.tokens[1].metadata = Some(temp_metadata);

        // owner deposits token_b
        testing_env!(get_owner_ft_transfer_context(owner(), token_b(), false));
        amm.ft_on_transfer(owner(), TEST_AMOUNT, "".to_string());
        assert_eq!(amm.get_balance(token_b()), TEST_AMOUNT);

        // ratio gets updated accordingly and calculation does not overflow
        assert_eq!(
            amm.get_ratio(),
            near_sdk::json_types::U128(289_480_223_093_290_488_503_844_922_296_628_310_727)
        );

        // swap calculation does not overflow
        testing_env!(get_owner_ft_transfer_context(alice(), token_b(), false));
        amm.ft_on_transfer(alice(), TEST_AMOUNT, "".to_string());
    }
}
