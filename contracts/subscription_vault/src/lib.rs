#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, token, Address, Env, Symbol};

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    BelowMinimumTopup = 402,
    InvalidAmount = 403,
    InsufficientAllowance = 405,
    TransferFailed = 406,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    InsufficientBalance = 3,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    /// Wallet that owns and funds this subscription.
    pub subscriber: Address,
    /// Wallet that receives periodic charges.
    pub merchant: Address,
    /// Billing amount charged per interval in token base units.
    pub amount: i128,
    /// Length of each billing interval in seconds.
    pub interval_seconds: u64,
    /// Ledger timestamp of the last successful payment lifecycle event.
    pub last_payment_timestamp: u64,
    /// Current subscription status.
    pub status: SubscriptionStatus,
    /// Subscriber funds currently held in the vault for this subscription.
    pub prepaid_balance: i128,
    /// If true, usage-based add-ons may be charged by downstream logic.
    pub usage_enabled: bool,
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Initialize the vault with token/admin config and minimum top-up threshold.
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        if min_topup <= 0 {
            return Err(Error::InvalidAmount);
        }
        env.storage().instance().set(&Symbol::new(&env, "token"), &token);
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
        env.storage().instance().set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    /// 
    /// # Arguments
    /// * `min_topup` - Minimum amount (in token base units) required for deposit_funds.
    ///                 Prevents inefficient micro-deposits. Typical range: 1-10 USDC (1_000000 - 10_000000 for 6 decimals).
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        if min_topup <= 0 {
            return Err(Error::InvalidAmount);
        }
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&Symbol::new(&env, "admin")).ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        env.storage().instance().set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage().instance().get(&Symbol::new(&env, "min_topup")).ok_or(Error::NotFound)
    }

    /// Create a new subscription and pull initial prepaid funds into the vault.
    ///
    /// `amount` is both the recurring charge amount and the required initial prepaid deposit.
    /// The subscriber must approve this contract as spender on the token contract before calling.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        if amount <= 0 || interval_seconds == 0 {
            return Err(Error::InvalidAmount);
        }

        let token_address: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;
        let token_client = token::Client::new(&env, &token_address);
        let contract_address = env.current_contract_address();

        let allowance = token_client.allowance(&subscriber, &contract_address);
        if allowance < amount {
            return Err(Error::InsufficientAllowance);
        }

        let balance = token_client.balance(&subscriber);
        if balance < amount {
            return Err(Error::TransferFailed);
        }

        token_client.transfer_from(&contract_address, &subscriber, &contract_address, &amount);
        let now = env.ledger().timestamp();
        let sub = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: now,
            status: SubscriptionStatus::Active,
            prepaid_balance: amount,
            usage_enabled,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    /// Subscriber deposits more USDC into their vault for this subscription.
    /// 
    /// # Minimum top-up enforcement
    /// Rejects deposits below the configured minimum threshold to prevent inefficient
    /// micro-transactions that waste gas and complicate accounting. The minimum is set
    /// globally at contract initialization and adjustable by admin via `set_min_topup`.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscriber.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        
        let min_topup: i128 = env.storage().instance().get(&Symbol::new(&env, "min_topup")).ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }
        
        // TODO: transfer USDC from subscriber, increase prepaid_balance for subscription_id
        let _ = (env, subscription_id, amount);
        Ok(())
    }

    /// Billing engine (backend) calls this to charge one interval. Deducts from vault, pays merchant.
    pub fn charge_subscription(_env: Env, _subscription_id: u32) -> Result<(), Error> {
        // TODO: require_caller admin or authorized billing service
        // TODO: load subscription, check interval and balance, transfer to merchant, update last_payment_timestamp and prepaid_balance
        Ok(())
    }

    /// Subscriber or merchant cancels the subscription. Remaining balance can be withdrawn by subscriber.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        // TODO: load subscription, set status Cancelled, allow withdraw of prepaid_balance
        let _ = (env, subscription_id);
        Ok(())
    }

    /// Pause subscription (no charges until resumed).
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        // TODO: load subscription, set status Paused
        let _ = (env, subscription_id);
        Ok(())
    }

    /// Merchant withdraws accumulated USDC to their wallet.
    pub fn withdraw_merchant_funds(
        _env: Env,
        merchant: Address,
        _amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        // TODO: deduct from merchant's balance in contract, transfer token to merchant
        Ok(())
    }

    /// Read subscription by id (for indexing and UI).
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)
    }

    fn _next_id(env: &Env) -> u32 {
        let key = Symbol::new(env, "next_id");
        let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(id + 1));
        id
    }
}

#[cfg(test)]
mod test;
