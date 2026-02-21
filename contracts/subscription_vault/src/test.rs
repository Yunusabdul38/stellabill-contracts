use crate::{Error, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

// ── helpers ──────────────────────────────────────────────────────────────────

fn setup_contract(env: &Env) -> (SubscriptionVaultClient, Address, Address) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128); // 1 USDC min_topup
    (client, token, admin)
}

// ── existing tests (updated for new expiration field) ─────────────────────────

#[test]
fn test_init_and_struct() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128; // 1 USDC
    client.init(&token, &admin, &min_topup);

    assert_eq!(client.get_min_topup(), min_topup);
}

#[test]
fn test_subscription_struct() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: 10_000_0000, // 10 USDC (6 decimals)
        interval_seconds: 30 * 24 * 60 * 60, // 30 days
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
        expiration: None, // no fixed end date
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.expiration, None);
}

#[test]
fn test_subscription_struct_with_expiration() {
    let env = Env::default();
    let exp_ts: u64 = 1_800_000_000;
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: 10_000_0000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
        expiration: Some(exp_ts),
    };
    assert_eq!(sub.expiration, Some(exp_ts));
}

#[test]
fn test_min_topup_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token, &admin, &min_topup);

    let result = client.try_deposit_funds(&0, &subscriber, &4_999999);
    assert!(result.is_err());
}

#[test]
fn test_min_topup_exactly_at_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token, &admin, &min_topup);

    let result = client.try_deposit_funds(&0, &subscriber, &min_topup);
    assert!(result.is_ok());
}

#[test]
fn test_min_topup_above_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token, &admin, &min_topup);

    let result = client.try_deposit_funds(&0, &subscriber, &10_000000);
    assert!(result.is_ok());
}

#[test]
fn test_set_min_topup_by_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let initial_min = 1_000000i128;
    let new_min = 10_000000i128;

    client.init(&token, &admin, &initial_min);
    assert_eq!(client.get_min_topup(), initial_min);

    client.set_min_topup(&admin, &new_min);
    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_set_min_topup_unauthorized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let min_topup = 1_000000i128;

    client.init(&token, &admin, &min_topup);

    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert!(result.is_err());
}

// ── expiration tests ───────────────────────────────────────────────────────────

/// Creating a subscription with no expiration stores `None` in the vault.
#[test]
fn test_create_subscription_no_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );

    let sub = client.get_subscription(&id);
    assert_eq!(sub.expiration, None);
}

/// Creating a subscription with a future expiration stores the timestamp correctly.
#[test]
fn test_create_subscription_with_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    // Ledger starts at 0; set expiration well in the future.
    let exp_ts: u64 = 90 * 24 * 60 * 60; // 90 days

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &Some(exp_ts),
    );

    let sub = client.get_subscription(&id);
    assert_eq!(sub.expiration, Some(exp_ts));
}

/// Charging a subscription whose expiration is in the past returns SubscriptionExpired.
#[test]
fn test_charge_expired_subscription() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let exp_ts: u64 = 1_000; // expires at ledger second 1000

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &Some(exp_ts),
    );

    // Advance ledger past the expiration timestamp.
    env.ledger().set_timestamp(exp_ts + 1);

    let result = client.try_charge_subscription(&id);
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionExpired))),
        "expected SubscriptionExpired, got {:?}",
        result
    );
}

/// At the exact expiration boundary (timestamp == expiration) the charge is rejected.
#[test]
fn test_charge_at_exact_expiration_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let exp_ts: u64 = 5_000;

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &Some(exp_ts),
    );

    // Set ledger timestamp exactly to expiration — still rejected (>= check).
    env.ledger().set_timestamp(exp_ts);

    let result = client.try_charge_subscription(&id);
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionExpired))),
        "expected SubscriptionExpired at exact boundary, got {:?}",
        result
    );
}

/// One second before expiration the charge succeeds (stub returns Ok).
#[test]
fn test_charge_one_second_before_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let exp_ts: u64 = 5_000;

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &Some(exp_ts),
    );

    // One second before the expiration — charge should be allowed.
    env.ledger().set_timestamp(exp_ts - 1);

    let result = client.try_charge_subscription(&id);
    assert!(
        result.is_ok(),
        "expected Ok before expiration, got {:?}",
        result
    );
}

/// A subscription with no expiration can always be charged regardless of the ledger timestamp.
#[test]
fn test_charge_no_expiration_always_allowed() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None, // no expiration
    );

    // Advance to a very large timestamp — should still succeed.
    env.ledger().set_timestamp(u64::MAX / 2);

    let result = client.try_charge_subscription(&id);
    assert!(
        result.is_ok(),
        "expected Ok for subscription with no expiration, got {:?}",
        result
    );
}

/// charge_subscription on a non-existent ID returns NotFound.
#[test]
fn test_charge_nonexistent_subscription() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let result = client.try_charge_subscription(&999);
    assert!(
        matches!(result, Err(Ok(Error::NotFound))),
        "expected NotFound, got {:?}",
        result
    );
}

/// Long-running subscription (no expiration, many simulated intervals) always succeeds.
#[test]
fn test_long_running_no_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );

    // Simulate 5 years of monthly charges (ledger advances each time).
    let one_month: u64 = 30 * 24 * 60 * 60;
    for month in 1u64..=60 {
        env.ledger().set_timestamp(month * one_month);
        let result = client.try_charge_subscription(&id);
        assert!(result.is_ok(), "month {} failed: {:?}", month, result);
    }
}
