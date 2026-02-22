//! Single charge logic (no auth). Used by charge_subscription and batch_charge.
//!
//! **PRs that only change how one subscription is charged should edit this file only.**

use crate::queries::get_subscription;
use crate::state_machine::validate_status_transition;
use crate::types::{Error, SubscriptionStatus};
use soroban_sdk::Env;

pub fn charge_one(env: &Env, subscription_id: u32) -> Result<(), Error> {
    let mut sub = get_subscription(env, subscription_id)?;

    if sub.status != SubscriptionStatus::Active {
        return Err(Error::NotActive);
    }

    let now = env.ledger().timestamp();
    let next_allowed = sub
        .last_payment_timestamp
        .checked_add(sub.interval_seconds)
        .ok_or(Error::Overflow)?;
    if now < next_allowed {
        return Err(Error::IntervalNotElapsed);
    }

    if sub.prepaid_balance < sub.amount {
        validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.storage().instance().set(&subscription_id, &sub);
        return Err(Error::InsufficientBalance);
    }

    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_sub(sub.amount)
        .ok_or(Error::Overflow)?;
    sub.last_payment_timestamp = now;
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

/// Debit a metered `usage_amount` from a subscription's prepaid balance.
///
/// Shared safety checks:
/// * Subscription must exist (`NotFound`).
/// * Subscription must be `Active` (`NotActive`).
/// * `usage_enabled` must be `true` (`UsageNotEnabled`).
/// * `usage_amount` must be positive (`InvalidAmount`).
/// * `prepaid_balance >= usage_amount` (`InsufficientPrepaidBalance`).
///
/// On success the prepaid balance is reduced.  If the balance reaches zero
/// the subscription transitions to `InsufficientBalance`, blocking further
/// charges until the subscriber tops up.
pub fn charge_usage_one(env: &Env, subscription_id: u32, usage_amount: i128) -> Result<(), Error> {
    let mut sub = get_subscription(env, subscription_id)?;

    if sub.status != SubscriptionStatus::Active {
        return Err(Error::NotActive);
    }

    if !sub.usage_enabled {
        return Err(Error::UsageNotEnabled);
    }

    if usage_amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    if sub.prepaid_balance < usage_amount {
        return Err(Error::InsufficientPrepaidBalance);
    }

    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_sub(usage_amount)
        .ok_or(Error::Overflow)?;

    // If the vault is now empty, transition to InsufficientBalance so no
    // further charges (interval or usage) can proceed until top-up.
    if sub.prepaid_balance == 0 {
        validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
        sub.status = SubscriptionStatus::InsufficientBalance;
    }

    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}
