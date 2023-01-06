# Constant Product AMM With Two Tokens

The user can swap a specific number of tokens A for a number of tokens B, or vice-versa.
The AMM makes use of a constant product `K = A * B`, where A and B are the balances of tokens
A and B respectively, to calculate the exchange rate of the tokens.

On each swap, the product `K` must be constant, and the amounts are calculated using the
following formulas:

$amountOut_a = (balance_a * amountIn\_b) / (balance_b + amountIn\_b)$
$amountOut_b = (balance_b * amountIn\_a) / (balance_a + amountIn\_a)$

## Quick Start

This app was initialized with [create-near-app](https://github.com/near/create-near-app)

If you haven't installed dependencies during setup:

```bash
    npm install
```


Build and deploy your contract to TestNet with a temporary dev account:

```bash
    npm run deploy
```

Test the contract (this will run both the unit tests and the integration tests):

```bash
    npm test
```



## Exploring The Code

1. The AMM smart-contract code lives in the `/contract` folder. See the README there for
   more info.
2. For testing purposes, a test fungible token exists in `/test_token`. It has been adapted
   from [Ref](https://github.com/ref-finance/ref-contracts/blob/main/test-token/src/lib.rs).
3. Integration tests exist in `/integration-tests`. The AMM uses cross-contract calls that
   can't be tested with unit tests.

## AMM Design & Implementation

### Structures

#### Token Structure
The AMM stores the token information in a Token struct:

```rust
    #[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
    struct Token {
        address: AccountId,
        balance: Balance,
        metadata: Option<TokenMetadata>,
    }
```

The `metadata` field is an Option to easily check whether the metadata has been stored.

#### Metadata Structure

The `Metadata` struct stores the address of the token name, the ticker symbol, and the decimals.
The definition is as follows:

```rust
    #[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Clone, PanicOnDefault)]
    pub struct TokenMetadata {
        name: String,
        symbol: String,
        decimals: u8,
    }
```

Each struct has a simple corresponding implementation of an initialization function called `new`.

#### AMM Structure

```rust
    #[near_bindgen]
    #[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
    pub struct AMM {
        owner: AccountId,
        tokens: Vec<Token>,
    }
```

The AMM structure holds an owner, which is the account that can freely deposit tokens in the AMM and modify
the token ratio.

The tokens are stored in a vector, which will always have a size of 2. This design choice will be explained
later, in the AMM implementation section.

### AMM Implementation

The initialization function takes the owner and the two tokens as parameters.

```rust
    #[init]
    pub fn new(owner: AccountId, token_a: AccountId, token_b: AccountId) -> Self {
        let amm = Self {
            owner,
            tokens: vec![Token::new(token_a.clone()), Token::new(token_b.clone())],
        };

        amm.update_metadata(&token_a);
        amm.update_metadata(&token_b);

        amm
    }
```

The metadata will then get requested with the help of cross-contract calls to the tokens' ft_metadata function.

```rust
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
```
If retrieving the metadata for a specific token fails, the function can be called again.

Then the metadata will be received and stored in the callback function shown below:

```rust
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
```

In order for the owner to deposit tokens in the AMM and modify the ratio, the AMM contract has been
made a `FungibleTokenReceiver` and is implementing the `ft_on_transfer` function which gets called
by the deposited token's smart contract whenever a transfer is done using `ft_transfer_call`.

However, if a user that is not the owner deposits tokens using `ft_transfer_call`, then the action will
be interpreted as a swap.

Here is the implementation of the `ft_on_transfer` function:
```rust
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
```

The function must return the amount of tokens that have to be reimbursed to the caller and the reimbursement
will be handled by the token's contract.

If the owner deposits tokens, then the `ownder_deposit` function is called and it updates the token balance,
without performing a swap. This will also modify the ratio. Since all the tokens are used, no
reimbursement has to be done so the value returned is 0.

```rust
    fn owner_deposit(&mut self, token_in: usize, amount: Balance) {
        self.tokens[token_in].balance += amount;
    }
```

The swap function must calculate the amount of tokens that must be received after a swap is performed.
However, the intermediate calculations can overflow, even if the final result fits in `u128`. Therefore,
the `construct_uint!` macro is used to create a 256 bits data type `U256` (more details here:
https://crates.io/crates/uint), which is used for the calculations that can overflow.

```rust
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
                    .update_balances_callback(new_balance_a, new_balance_b, amount),
            )
    }
```

After the calculations are done, another cross-contract call must be performed to transfer the
swapped tokens and the AMM's balances must be updated.

However, the transfer cross-contract call can fail, for example, if the user is unregistered
with the swapped token's storage. If this happens, the user's funds will be lost in the AMM and
the AMM won't be able to transfer the swapped tokens. Therefore, the callback `swap_callback`
must handle this case and inform the calling contract to refund the transferred tokens:

```rust
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
```

Referring to tokens by index provides the possibility to refer to tokens by index which helps
reduce code duplication (for example, swapping from token A to token B and vice-versa can simply
be done by switching the order of the provided indices, without having to write separate functions
for each type of swap).

The AMM smart contract also implements getter functions for:

1. ratio
2. token balances
3. token metadata

## Testing

The testing is done using a combination of unit tests and integration tests. Because of the
cross-contract calls, unit tests can't thoroughly test the functionality and security of the smart
contract and slower, more robust integration tests have been employed. Both kinds of tests can be
run using `npm test`.

The unit tests have been written in the same file as the AMM smart contract, in `/contract/src/lib.rs`
and the integration tests exist in `/integration-tests/src/tests.rs`.

The integration tests make use of a utility file (in `/integration-tests/src/test_utils.rs`) consisting
of commonly called functions for interacting with the underlying contracts.

The scenarios tested are as follows:

1. initialization works as expected
2. ratio can't be calculated without metadata (because the decimals are unknown)
3. swapping with/for 0 tokens fails
4. calculations do not overflow (thanks to U256)
5. balances and ratio gets updated correctly
6. `ft_transfer` calls do not affect the ratio/balances (it can only be done
   using `ft_on_transfer`)
7. failed swaps reimburse the deposited tokens
8. swaps work as expected
9. attempting to swap with unsupported tokens fails and the AMM state does not change

## Deploy

Every smart contract in NEAR has its [own associated account](https://docs.near.org/concepts/basics/account).
When you run `npm run deploy`, your smart contract gets deployed to the live NEAR TestNet with a temporary dev account.
When you're ready to make it permanent, here's how:


#### Step 0: Install near-cli (optional)

[near-cli](https://github.com/near/near-cli) is a command line interface (CLI) for interacting with the NEAR blockchain. It was installed to the local `node_modules` folder when you ran `npm install`, but for best ergonomics, you may want to install it globally:
```bash
    npm install --global near-cli
```

Or, if you'd rather use the locally-installed version, you can prefix all `near` commands with `npx`

Ensure that it's installed with `near --version` (or `npx near --version`)


#### Step 1: Create an account for the contracts

Each account on NEAR can have at most one contract deployed to it. If you've already created an account such as `your-name.testnet`, you can deploy your contract to `near-blank-project.your-name.testnet`. Assuming you've already created an account on [NEAR Wallet](https://wallet.testnet.near.org/), here's how to create `near-blank-project.your-name.testnet`:

1. Authorize NEAR CLI, by following the commands it gives you:

```bash
      near login
```

2. Create a subaccount (replace `YOUR-NAME` below with your actual account name):

```bash
      near create-account near-blank-project.YOUR-NAME.testnet --masterAccount YOUR-NAME.testnet
```

#### Step 2: deploy the contracts

Use the CLI to deploy the contract to TestNet with your account ID.
Replace `PATH_TO_WASM_FILE` with the `wasm` that was generated in `contract` build directory.

```bash
    near deploy --accountId near-blank-project.YOUR-NAME.testnet --wasmFile PATH_TO_WASM_FILE
```

### Interacting with AMM through CLI:

If you plan on playing around with the AMM, you will also have to deploy test tokens or use ones that
have already been deployed.

For example:

0. Create accounts:

```bash
near create-account amm.YOUR-NAME.testnet --masterAccount YOUR-NAME.testnet
near create-account token_a.YOUR-NAME.testnet --masterAccount YOUR-NAME.testnet
near create-account token_b.YOUR-NAME.testnet --masterAccount YOUR-NAME.testnet
near create-account test_user.YOUR-NAME.testnet --masterAccount YOUR-NAME.testnet
```

1. Deploy the contracts:

```bash
near deploy --accountId amm.YOUR-NAME.testnet --wasmFile ./contract/target/wasm32-unknown-unknown/release/amm.wasm
near deploy --accountId token_a.YOUR-NAME.testnet --wasmFile ./test_token/target/wasm32-unknown-unknown/release/test_token.wasm
near deploy --accountId token_b.YOUR-NAME.testnet --wasmFile ./test_token/target/wasm32-unknown-unknown/release/test_token.wasm
```

If it can't find `./test_token/target/wasm32-unknown-unknown/release/test_token.wasm`, run:

```bash
cd test-token && sh build.sh && cd ..
```
Then try again. Similarly for `amm.wasm`.

2. Initialize the test tokens:

```bash
near call token_a.YOUR-NAME.testnet new '{"name": "Token A", "decimals": 8}' --account-id YOUR-NAME.testnet --gas=300000000000000
near call token_b.YOUR-NAME.testnet new '{"name": "Token B", "decimals": 16}' --account-id YOUR-NAME.testnet --gas=300000000000000
```
3. Initialize the AMM:

```bash
near call amm.YOUR-NAME.testnet new '{"owner": "YOUR-NAME.testnet", "token_a": "token_a.YOUR-NAME.testnet", "token_b": "token_b.YOUR-NAME.testnet"}' --account-id YOUR-NAME.testnet --gas=300000000000000
```
4. Register the AMM with the token storages:

```bash
near call token_a.YOUR-NAME.testnet storage_deposit '{"account_id": "amm.YOUR-NAME.testnet"}' --accountId YOUR-NAME.testnet --amount 0.00125
near call token_b.YOUR-NAME.testnet storage_deposit '{"account_id": "amm.YOUR-NAME.testnet"}' --accountId YOUR-NAME.testnet --amount 0.00125
```
5. Mint test tokens to the owner address:

```bash
near call token_a.YOUR-NAME.testnet storage_deposit '{"account_id": "YOUR-NAME.testnet", "amount": "1000000000000000000"}' --accountId YOUR-NAME.testnet --amount 0.00125
near call token_b.YOUR-NAME.testnet storage_deposit '{"account_id": "YOUR-NAME.testnet", "amount": "1000000000000000000"}' --accountId YOUR-NAME.testnet --amount 0.00125
```
6. Deposit tokens in the AMM:

```bash
near call token_a.YOUR-NAME.testnet ft_transfer_call '{"receiver_id": "amm.YOUR-NAME.testnet", "amount": "10000000000000000", "msg": ""}' --accountId YOUR-NAME.testnet --depositYocto 1
near call token_b.YOUR-NAME.testnet ft_transfer_call '{"receiver_id": "amm.YOUR-NAME.testnet", "amount": "100000000000000000", "msg": ""}' --accountId YOUR-NAME.testnet --depositYocto 1
```
7. View balances/ratio:

```bash
near view amm.YOUR-NAME.testnet get_balance '{"token": "token_a.YOUR-NAME.testnet"}'  --account-id YOUR-NAME.testnet
near view amm.YOUR-NAME.testnet get_balance '{"token": "token_b.YOUR-NAME.testnet"}'  --account-id YOUR-NAME.testnet
near view amm.YOUR-NAME.testnet get_ratio ''  --account-id YOUR-NAME.testnet
```

8. Mint tokens for the test user (the minting function also registers the user with the storage):

```bash
near call token_a.YOUR-NAME.testnet storage_deposit '{"account_id": "test_user.YOUR-NAME.testnet", "amount": "10000000000000000"}' --accountId test_user.YOUR-NAME.testnet --amount 0.00125
```

9. Register the test user with the other token's storage:

```bash
near call token_b.YOUR-NAME.testnet storage_deposit '{"account_id": "test_user.YOUR-NAME.testnet"}' --accountId test_user.YOUR-NAME.testnet --amount 0.00125
```

10. Perform a swap with the test user:

```bash
near call token_a.YOUR-NAME.testnet ft_transfer_call '{"receiver_id": "amm.YOUR-NAME.testnet", "amount": "10000000000000000", "msg": ""}' --accountId test_user.YOUR-NAME.testnet --depositYocto 1
```

There are more scenarios covered in the integration tests.

## Troubleshooting

On Windows, if you're seeing an error containing `EPERM` it may be related to spaces in your path. Please see [this issue](https://github.com/zkat/npx/issues/209) for more details.
