//! Subscription lifecycle: create, deposit, charge, cancel, pause, resume.
//!
//! **PRs that only change subscription lifecycle or billing should edit this file only.**

use crate::admin::require_admin;
use crate::charge_core::charge_one;
use crate::queries::get_subscription;
use crate::state_machine::validate_status_transition;
use crate::types::{Error, PlanTemplate, Subscription, SubscriptionStatus};
use soroban_sdk::{Address, Env, Symbol};

pub fn next_id(env: &Env) -> u32 {
    let key = Symbol::new(env, "next_id");
    let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
    env.storage().instance().set(&key, &(id + 1));
    id
}

pub fn next_plan_id(env: &Env) -> u32 {
    let key = Symbol::new(env, "next_plan_id");
    let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
    env.storage().instance().set(&key, &(id + 1));
    id
}

pub fn get_plan_template(env: &Env, plan_template_id: u32) -> Result<PlanTemplate, Error> {
    let key = (Symbol::new(env, "plan"), plan_template_id);
    env.storage().instance().get(&key).ok_or(Error::NotFound)
}

pub fn do_create_subscription(
    env: &Env,
    subscriber: Address,
    merchant: Address,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
) -> Result<u32, Error> {
    subscriber.require_auth();
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant,
        amount,
        interval_seconds,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled,
    };
    let id = next_id(env);
    env.storage().instance().set(&id, &sub);
    Ok(id)
}

pub fn do_deposit_funds(
    env: &Env,
    subscription_id: u32,
    subscriber: Address,
    amount: i128,
) -> Result<(), Error> {
    subscriber.require_auth();

    let min_topup: i128 = crate::admin::get_min_topup(env)?;
    if amount < min_topup {
        return Err(Error::BelowMinimumTopup);
    }

    let mut sub = get_subscription(env, subscription_id)?;
    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

pub fn do_charge_subscription(env: &Env, subscription_id: u32) -> Result<(), Error> {
    let admin = require_admin(env)?;
    admin.require_auth();
    charge_one(env, subscription_id)
}

pub fn do_cancel_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;
    validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
    sub.status = SubscriptionStatus::Cancelled;
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

pub fn do_pause_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;
    validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;
    sub.status = SubscriptionStatus::Paused;
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

pub fn do_resume_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;
    validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
    sub.status = SubscriptionStatus::Active;
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

pub fn do_create_plan_template(
    env: &Env,
    merchant: Address,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
) -> Result<u32, Error> {
    merchant.require_auth();

    let plan = PlanTemplate {
        merchant,
        amount,
        interval_seconds,
        usage_enabled,
    };

    let plan_id = next_plan_id(env);
    let key = (Symbol::new(env, "plan"), plan_id);
    env.storage().instance().set(&key, &plan);

    Ok(plan_id)
}

pub fn do_create_subscription_from_plan(
    env: &Env,
    subscriber: Address,
    plan_template_id: u32,
) -> Result<u32, Error> {
    subscriber.require_auth();

    let plan = get_plan_template(env, plan_template_id)?;

    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant: plan.merchant,
        amount: plan.amount,
        interval_seconds: plan.interval_seconds,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled: plan.usage_enabled,
    };

    let id = next_id(env);
    env.storage().instance().set(&id, &sub);
    Ok(id)
}
