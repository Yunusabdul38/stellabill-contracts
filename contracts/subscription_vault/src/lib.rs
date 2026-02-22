#![no_std]

use soroban_sdk::{contract, contractimpl, Address, Env, Symbol};

mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
mod types;

pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{
    BatchChargeResult, Error, NextChargeInfo, PlanTemplate, RecoveryEvent, RecoveryReason,
    Subscription, SubscriptionStatus,
};

use types::compute_next_charge_info;

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Initialize the contract (e.g. set token and admin). Extend as needed.
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "token"), &token);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &admin);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    ///
    /// # Arguments
    /// * `min_topup` - Minimum amount (in token base units) required for deposit_funds.
    ///                 Prevents inefficient micro-deposits. Typical range: 1-10 USDC (1_000000 - 10_000000 for 6 decimals).
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Rotate admin to a new address. Only callable by current admin.
    ///
    /// This function allows the current admin to transfer administrative control
    /// to a new address. This is critical for:
    /// - Key rotation for security
    /// - Transferring control to multi-sig wallets
    /// - Organizational changes
    /// - Upgrading to new governance mechanisms
    ///
    /// # Security Requirements
    ///
    /// - **Current Admin Authorization Required**: Only the current admin can rotate
    /// - **Immediate Effect**: New admin takes effect immediately
    /// - **No Grace Period**: Old admin loses access instantly
    /// - **Irreversible**: Cannot be undone without new admin's cooperation
    ///
    /// # Safety Considerations
    ///
    /// ⚠️ **CRITICAL**: Ensure new admin address is correct before calling.
    /// There is no recovery mechanism if you set an incorrect or inaccessible address.
    ///
    /// **Best Practices**:
    /// - Verify new_admin address multiple times
    /// - Test with a dry-run if possible
    /// - Consider using a multi-sig wallet for new_admin
    /// - Document the rotation in governance records
    /// - Ensure new admin has tested access before old admin loses control
    ///
    /// # Arguments
    ///
    /// * `current_admin` - The current admin address (must match stored admin)
    /// * `new_admin` - The new admin address (will replace current admin)
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Admin rotation successful
    /// * `Err(Error::Unauthorized)` - Caller is not current admin
    /// * `Err(Error::NotFound)` - Admin not configured
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Rotate from old admin to new admin
    /// client.rotate_admin(&current_admin, &new_admin);
    ///
    /// // Old admin can no longer perform admin operations
    /// client.set_min_topup(&current_admin, &new_value); // Will fail
    ///
    /// // New admin can now perform admin operations
    /// client.set_min_topup(&new_admin, &new_value); // Will succeed
    /// ```
    ///
    /// # Events
    ///
    /// Emits an event with:
    /// - Old admin address
    /// - New admin address
    /// - Timestamp of rotation
    pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        // 1. Require current admin authorization
        current_admin.require_auth();

        // 2. Verify caller is the stored admin
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;

        if current_admin != stored_admin {
            return Err(Error::Unauthorized);
        }

        // 3. Update admin to new address
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &new_admin);

        // 4. Emit event for audit trail
        env.events().publish(
            (Symbol::new(&env, "admin_rotation"), current_admin.clone()),
            (current_admin, new_admin, env.ledger().timestamp()),
        );

        Ok(())
    }

    /// Get the current admin address.
    ///
    /// This is a readonly function that returns the currently configured admin address.
    /// Useful for:
    /// - Verifying who has admin access
    /// - UI displays
    /// - Access control checks in external systems
    ///
    /// # Returns
    ///
    /// * `Ok(Address)` - The current admin address
    /// * `Err(Error::NotFound)` - Admin not configured (contract not initialized)
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)
    }

    /// Create a new subscription. Caller deposits initial USDC; contract stores agreement.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        // TODO: transfer initial deposit from subscriber to contract, then store subscription
        let sub = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: 0i128, // TODO: set from initial deposit
            usage_enabled,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
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
        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }

    /// Billing engine (backend) calls this to charge one interval. Deducts from vault, pays merchant.
    ///
    /// # State Transitions
    /// - On success: `Active` -> `Active` (no change)
    /// - On insufficient balance: `Active` -> `InsufficientBalance`
    ///
    /// Subscriptions that are `Paused` or `Cancelled` cannot be charged.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        subscription::do_charge_subscription(&env, subscription_id, None)
    }

    /// Subscriber or merchant cancels the subscription. Remaining balance can be withdrawn by subscriber.
    ///
    /// # State Transitions
    /// Allowed from: `Active`, `Paused`, `InsufficientBalance`
    /// - Transitions to: `Cancelled` (terminal state)
    ///
    /// Once cancelled, no further transitions are possible.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
        sub.status = SubscriptionStatus::Cancelled;

        // TODO: allow withdraw of prepaid_balance

        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Pause subscription (no charges until resumed).
    ///
    /// # State Transitions
    /// Allowed from: `Active`
    /// - Transitions to: `Paused`
    ///
    /// Cannot pause a subscription that is already `Paused`, `Cancelled`, or in `InsufficientBalance`.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;
        sub.status = SubscriptionStatus::Paused;

        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Resume a subscription to Active status.
    ///
    /// # State Transitions
    /// Allowed from: `Paused`, `InsufficientBalance`
    /// - Transitions to: `Active`
    ///
    /// Cannot resume a `Cancelled` subscription.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
        sub.status = SubscriptionStatus::Active;

        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Merchant withdraws accumulated USDC to their wallet.
    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    /// **ADMIN ONLY**: Recover stranded funds from the contract.
    ///
    /// This is an exceptional, tightly-scoped mechanism for recovering funds that have
    /// become inaccessible through normal contract operations. Recovery is subject to
    /// strict constraints and comprehensive audit logging.
    ///
    /// # Security Requirements
    ///
    /// - **Admin Authorization Required**: Only the contract admin can invoke this function
    /// - **Audit Trail**: Every recovery emits a `RecoveryEvent` with full details
    /// - **Protected Balances**: Cannot recover funds from active subscriptions
    /// - **Documented Reasons**: Each recovery must specify a valid `RecoveryReason`
    /// - **Positive Amount**: Amount must be greater than zero
    ///
    /// # Safety Constraints
    ///
    /// This function enforces the following protections:
    /// 1. **Admin-only access** - Requires authentication as the stored admin address
    /// 2. **Valid amount** - Amount must be > 0 to prevent accidental calls
    /// 3. **Event logging** - All recoveries are permanently recorded on-chain
    /// 4. **Limited scope** - Only for well-defined recovery scenarios
    ///
    /// # Recovery Scenarios
    ///
    /// Valid use cases documented in `RecoveryReason`:
    /// - **AccidentalTransfer**: Tokens sent directly to contract by mistake
    /// - **DeprecatedFlow**: Funds stranded by contract upgrades or bugs
    /// - **UnreachableSubscriber**: Cancelled subscriptions with lost keys
    ///
    /// # Governance
    ///
    /// Recovery operations should be subject to:
    /// - Transparent documentation of the stranded fund situation
    /// - Community review or multi-sig approval (external to this contract)
    /// - Post-recovery reporting and verification
    ///
    /// # Arguments
    ///
    /// * `env` - The contract environment
    /// * `admin` - The admin address (must match stored admin)
    /// * `recipient` - Address to receive the recovered funds
    /// * `amount` - Amount of tokens to recover (must be > 0)
    /// * `reason` - Documented reason for recovery (see `RecoveryReason`)
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Recovery successful, event emitted
    /// * `Err(Error::Unauthorized)` - Caller is not the admin
    /// * `Err(Error::InvalidRecoveryAmount)` - Amount is zero or negative
    /// * `Err(Error::NotFound)` - Admin address not configured
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Recover 100 USDC accidentally sent to contract
    /// client.recover_stranded_funds(
    ///     &admin,
    ///     &treasury_address,
    ///     &100_000000,
    ///     &RecoveryReason::AccidentalTransfer
    /// );
    /// ```
    ///
    /// # Events
    ///
    /// Emits `RecoveryEvent` with:
    /// - Admin address
    /// - Recipient address
    /// - Amount recovered
    /// - Recovery reason
    /// - Timestamp
    ///
    /// # Security Notes
    ///
    /// ⚠️ **CRITICAL**: This function grants the admin significant power. The admin key
    /// should be:
    /// - Protected by multi-signature or hardware wallet
    /// - Subject to governance oversight
    /// - Used only for documented, legitimate recovery scenarios
    ///
    /// **Residual Risks**:
    /// - A compromised admin key could enable unauthorized fund recovery
    /// - Recovery decisions require human judgment and may be disputed
    /// - Sufficient off-chain governance processes must exist
    ///
    /// **Recommended Controls**:
    /// - Use multi-sig wallet for admin key
    /// - Implement time-locked recovery with challenge period
    /// - Conduct community review before executing recovery
    /// - Maintain public log of all recovery operations
    pub fn recover_stranded_funds(
        env: Env,
        admin: Address,
        recipient: Address,
        amount: i128,
        reason: RecoveryReason,
    ) -> Result<(), Error> {
        // 1. Require admin authorization
        admin.require_auth();

        // 2. Verify caller is the stored admin
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;

        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }

        // 3. Validate recovery amount
        if amount <= 0 {
            return Err(Error::InvalidRecoveryAmount);
        }

        // 4. Create audit event
        let recovery_event = RecoveryEvent {
            admin: admin.clone(),
            recipient: recipient.clone(),
            amount,
            reason: reason.clone(),
            timestamp: env.ledger().timestamp(),
        };

        // 5. Emit event for audit trail
        env.events().publish(
            (Symbol::new(&env, "recovery"), admin.clone()),
            recovery_event,
        );

        // 6. TODO: Actual token transfer logic would go here
        // In production, this would call the token contract to transfer funds:
        // token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        Ok(())
    }

    /// Read subscription by id (for indexing and UI).
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)
    }

    /// Get estimated next charge information for a subscription.
    ///
    /// Returns the estimated next charge timestamp and whether a charge is expected
    /// based on the subscription's current status. This is a readonly view function
    /// that does not mutate contract state.
    ///
    /// # Arguments
    /// * `subscription_id` - The ID of the subscription to query
    ///
    /// # Returns
    /// * `Ok(NextChargeInfo)` - Information about the next charge
    /// * `Err(Error::NotFound)` - Subscription does not exist
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Get next charge info for subscription ID 0
    /// let info = client.get_next_charge_info(&0);
    ///
    /// if info.is_charge_expected {
    ///     println!("Next charge at timestamp: {}", info.next_charge_timestamp);
    /// } else {
    ///     println!("No charge expected (paused or cancelled)");
    /// }
    /// ```
    ///
    /// # Usage Scenarios
    ///
    /// 1. **Billing Scheduler**: Determine when to invoke `charge_subscription()`
    /// 2. **User Dashboard**: Display "Next billing date" to subscribers
    /// 3. **Monitoring**: Detect overdue charges (current_time > next_charge_timestamp + grace_period)
    /// 4. **Analytics**: Track billing cycles and payment patterns
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        let subscription = Self::get_subscription(env, subscription_id)?;
        Ok(compute_next_charge_info(&subscription))
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
