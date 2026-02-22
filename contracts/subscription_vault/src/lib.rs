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
    InsufficientBalance = 407,
    InvalidStatus = 408,
    ArithmeticOverflow = 409,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum DataKey {
    MerchantBalance(Address),
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
mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, Vec};

pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{
    BatchChargeResult, Error, FundsDepositedEvent, MerchantWithdrawalEvent, OneOffChargedEvent,
    Subscription, SubscriptionCancelledEvent, SubscriptionChargedEvent, SubscriptionCreatedEvent,
    SubscriptionPausedEvent, SubscriptionResumedEvent, SubscriptionStatus,
};

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
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_init(&env, token, admin, min_topup)
    }

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
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
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
        subscription::do_create_subscription(
            &env,
            subscriber,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
        )
    }

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

    /// Charge one billing interval and accrue earnings to the merchant's internal balance.
    ///
    /// On success this atomically:
    /// 1. debits `subscription.prepaid_balance` by `subscription.amount`
    /// 2. credits the merchant's aggregate balance ledger by the same amount
    /// 3. updates `last_payment_timestamp`
    ///
    /// Tokens are not transferred to the merchant here. They remain in the vault until
    /// `withdraw_merchant_funds` is called.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        let mut subscription: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        if subscription.status != SubscriptionStatus::Active {
            return Err(Error::InvalidStatus);
        }

        if subscription.prepaid_balance < subscription.amount {
            return Err(Error::InsufficientBalance);
        }

        let updated_prepaid = subscription
            .prepaid_balance
            .checked_sub(subscription.amount)
            .ok_or(Error::ArithmeticOverflow)?;
        let current_merchant_balance = Self::read_merchant_balance(&env, &subscription.merchant);
        let updated_merchant_balance = current_merchant_balance
            .checked_add(subscription.amount)
            .ok_or(Error::ArithmeticOverflow)?;

        subscription.prepaid_balance = updated_prepaid;
        subscription.last_payment_timestamp = env.ledger().timestamp();
        env.storage().instance().set(&subscription_id, &subscription);
        Self::write_merchant_balance(&env, &subscription.merchant, updated_merchant_balance);
        Ok(())
        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }

    /// Charge one subscription for the current billing interval. Optional `idempotency_key` enables
    /// safe retries: repeated calls with the same key return success without double-charging.
    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
        idempotency_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<(), Error> {
        subscription::do_charge_subscription(&env, subscription_id, idempotency_key)
    }

    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &subscription_ids)
    }

    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;

        let refund = sub.prepaid_balance;
        sub.status = SubscriptionStatus::Cancelled;
        env.storage().instance().set(&subscription_id, &sub);

        env.events().publish(
            (symbol_short!("cancelled"),),
            SubscriptionCancelledEvent {
                subscription_id,
                authorizer,
                refund_amount: refund,
            },
        );

        Ok(())
    }

    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;

        sub.status = SubscriptionStatus::Paused;
        env.storage().instance().set(&subscription_id, &sub);

        env.events().publish(
            (symbol_short!("paused"),),
            SubscriptionPausedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
    }

    /// Merchant withdraws accumulated USDC from their internal earned balance.
    ///
    /// This debits internal merchant earnings first and then transfers the same amount of
    /// tokens from vault custody to the merchant wallet. This prevents double spending across
    /// repeated withdraw calls.
    pub fn withdraw_merchant_funds(
        env: Env,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let current_balance = Self::read_merchant_balance(&env, &merchant);
        if current_balance < amount {
            return Err(Error::InsufficientBalance);
        }

        let updated_balance = current_balance
            .checked_sub(amount)
            .ok_or(Error::ArithmeticOverflow)?;
        Self::write_merchant_balance(&env, &merchant, updated_balance);

        let token_address: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;
        let token_client = token::Client::new(&env, &token_address);
        let contract_address = env.current_contract_address();
        token_client.transfer(&contract_address, &merchant, &amount);
        Ok(())
    }

    /// Returns the internal earned balance currently available for a merchant to withdraw.
    pub fn get_merchant_balance(env: Env, merchant: Address) -> Result<i128, Error> {
        Ok(Self::read_merchant_balance(&env, &merchant))
    }

    /// Read subscription by id (for indexing and UI).
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;

        sub.status = SubscriptionStatus::Active;
        env.storage().instance().set(&subscription_id, &sub);

        env.events().publish(
            (symbol_short!("resumed"),),
            SubscriptionResumedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
    }

    /// Merchant-initiated one-off charge: debits `amount` from the subscription's prepaid balance.
    /// Caller must be the subscription's merchant (requires auth). Amount must not exceed
    /// prepaid_balance; subscription must be Active or Paused.
    pub fn charge_one_off(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscription::do_charge_one_off(&env, subscription_id, merchant, amount)
    }

    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    fn read_merchant_balance(env: &Env, merchant: &Address) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::MerchantBalance(merchant.clone()))
            .unwrap_or(0i128)
    }

    fn write_merchant_balance(env: &Env, merchant: &Address, balance: i128) {
        env.storage()
            .instance()
            .set(&DataKey::MerchantBalance(merchant.clone()), &balance);
    }
}

#[cfg(test)]
mod test;
