#![no_std]

mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
mod types;

use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{BatchChargeResult, Error, PlanTemplate, Subscription, SubscriptionStatus};

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_init(&env, token, admin, min_topup)
    }

    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
    }

    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscription::do_create_subscription(
            &env,
            subscriber,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
        )
    }

    /// Creates a plan template that can be used to instantiate subscriptions.
    ///
    /// Plan templates allow merchants to define reusable subscription offerings
    /// with predefined parameters. This ensures consistency and reduces the need
    /// for repeated parameter input when creating similar subscriptions.
    ///
    /// # Arguments
    ///
    /// * `merchant` - The merchant address that owns this plan template
    /// * `amount` - The recurring charge amount per interval
    /// * `interval_seconds` - The billing interval in seconds
    /// * `usage_enabled` - Whether usage-based charging is enabled
    ///
    /// # Returns
    ///
    /// The unique plan template ID that can be used to create subscriptions
    ///
    /// # Example Use Cases
    ///
    /// - "Basic Plan": $9.99/month with standard features
    /// - "Premium Plan": $29.99/month with advanced features
    /// - "Enterprise Plan": Custom pricing with usage-based billing
    pub fn create_plan_template(
        env: Env,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscription::do_create_plan_template(
            &env,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
        )
    }

    /// Creates a subscription from a predefined plan template.
    ///
    /// This function instantiates a new subscription using the parameters defined
    /// in a plan template. The subscriber only needs to provide their address and
    /// the template ID, while all other parameters (amount, interval, usage settings)
    /// are inherited from the template.
    ///
    /// # Arguments
    ///
    /// * `subscriber` - The subscriber address for the new subscription
    /// * `plan_template_id` - The ID of the plan template to use
    ///
    /// # Returns
    ///
    /// The unique subscription ID for the newly created subscription
    ///
    /// # Benefits
    ///
    /// - Reduces parameter input errors
    /// - Ensures consistency across subscriptions using the same plan
    /// - Simplifies the subscription creation process for end users
    /// - Allows merchants to update plan offerings centrally
    pub fn create_subscription_from_plan(
        env: Env,
        subscriber: Address,
        plan_template_id: u32,
    ) -> Result<u32, Error> {
        subscription::do_create_subscription_from_plan(&env, subscriber, plan_template_id)
    }

    /// Retrieves a plan template by its ID.
    ///
    /// # Arguments
    ///
    /// * `plan_template_id` - The ID of the plan template to retrieve
    ///
    /// # Returns
    ///
    /// The plan template details
    pub fn get_plan_template(env: Env, plan_template_id: u32) -> Result<PlanTemplate, Error> {
        subscription::get_plan_template(&env, plan_template_id)
    }

    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }

    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        subscription::do_charge_subscription(&env, subscription_id)
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
        subscription::do_cancel_subscription(&env, subscription_id, authorizer)
    }

    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer)
    }

    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_resume_subscription(&env, subscription_id, authorizer)
    }

    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }
}

#[cfg(test)]
mod test;
