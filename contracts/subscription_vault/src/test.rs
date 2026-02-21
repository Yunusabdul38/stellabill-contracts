use crate::{Error, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{token, Address, Env};

struct TestCtx {
    env: Env,
    contract_id: Address,
    token_address: Address,
    admin: Address,
    subscriber: Address,
}

impl TestCtx {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token_address = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let token_admin_client = token::StellarAssetClient::new(&env, &token_address);

        let admin = Address::generate(&env);
        let subscriber = Address::generate(&env);

        client.init(&token_address, &admin, &1_000000i128);
        token_admin_client.mint(&subscriber, &100_000000i128);

        Self {
            env,
            contract_id,
            token_address,
            admin,
            subscriber,
        }
    }

    fn client(&self) -> SubscriptionVaultClient<'_> {
        SubscriptionVaultClient::new(&self.env, &self.contract_id)
    }

    fn token_client(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.token_address)
    }
}

#[test]
fn test_init_and_struct() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    assert_eq!(client.get_min_topup(), 1_000000i128);

    let sub = Subscription {
        subscriber: Address::generate(&ctx.env),
        merchant: Address::generate(&ctx.env),
        amount: 10_000_0000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
fn test_create_subscription_initializes_prepaid_and_transfers_tokens() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let token_client = ctx.token_client();
    let merchant = Address::generate(&ctx.env);
    let amount = 10_000000i128;

    token_client.approve(
        &ctx.subscriber,
        &ctx.contract_id,
        &amount,
        &ctx.env.ledger().sequence().saturating_add(500),
    );

    let before_subscriber = token_client.balance(&ctx.subscriber);
    let before_vault = token_client.balance(&ctx.contract_id);

    let sub_id = client.create_subscription(&ctx.subscriber, &merchant, &amount, &3600u64, &false);
    let sub = client.get_subscription(&sub_id);

    assert_eq!(sub.prepaid_balance, amount);
    assert_eq!(sub.last_payment_timestamp, ctx.env.ledger().timestamp());
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(token_client.balance(&ctx.subscriber), before_subscriber - amount);
    assert_eq!(token_client.balance(&ctx.contract_id), before_vault + amount);
}

#[test]
fn test_create_subscription_missing_allowance_fails() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    let result = client.try_create_subscription(&ctx.subscriber, &merchant, &5_000000i128, &3600u64, &false);
    assert_eq!(result, Err(Ok(Error::InsufficientAllowance)));
}

#[test]
fn test_create_subscription_transfer_failure_on_low_balance() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let token_client = ctx.token_client();
    let merchant = Address::generate(&ctx.env);
    let amount = 500_000000i128;

    token_client.approve(
        &ctx.subscriber,
        &ctx.contract_id,
        &amount,
        &ctx.env.ledger().sequence().saturating_add(500),
    );

    let result = client.try_create_subscription(&ctx.subscriber, &merchant, &amount, &3600u64, &true);
    assert_eq!(result, Err(Ok(Error::TransferFailed)));
}

#[test]
fn test_create_subscription_zero_or_negative_amount_fails() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let token_client = ctx.token_client();
    let merchant = Address::generate(&ctx.env);

    token_client.approve(
        &ctx.subscriber,
        &ctx.contract_id,
        &1_000000i128,
        &ctx.env.ledger().sequence().saturating_add(500),
    );

    let zero = client.try_create_subscription(&ctx.subscriber, &merchant, &0i128, &3600u64, &false);
    let negative = client.try_create_subscription(&ctx.subscriber, &merchant, &-1i128, &3600u64, &false);

    assert_eq!(zero, Err(Ok(Error::InvalidAmount)));
    assert_eq!(negative, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_create_subscription_zero_interval_fails() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let token_client = ctx.token_client();
    let merchant = Address::generate(&ctx.env);
    let amount = 1_000000i128;

    token_client.approve(
        &ctx.subscriber,
        &ctx.contract_id,
        &amount,
        &ctx.env.ledger().sequence().saturating_add(500),
    );

    let result = client.try_create_subscription(&ctx.subscriber, &merchant, &amount, &0u64, &false);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_min_topup_below_threshold() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    client.set_min_topup(&ctx.admin, &5_000000i128);

    let below = client.try_deposit_funds(&0, &ctx.subscriber, &4_999999i128);
    assert_eq!(below, Err(Ok(Error::BelowMinimumTopup)));

    let zero = client.try_deposit_funds(&0, &ctx.subscriber, &0i128);
    assert_eq!(zero, Err(Ok(Error::InvalidAmount)));

    let negative = client.try_deposit_funds(&0, &ctx.subscriber, &-100i128);
    assert_eq!(negative, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_min_topup_exactly_at_threshold() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let min_topup = 5_000000i128;
    client.set_min_topup(&ctx.admin, &min_topup);

    let result = client.try_deposit_funds(&0, &ctx.subscriber, &min_topup);
    assert!(result.is_ok());
}

#[test]
fn test_min_topup_above_threshold() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    client.set_min_topup(&ctx.admin, &5_000000i128);

    let result = client.try_deposit_funds(&0, &ctx.subscriber, &10_000000i128);
    assert!(result.is_ok());
}

#[test]
fn test_set_min_topup_by_admin() {
    let ctx = TestCtx::new();
    let client = ctx.client();

    assert_eq!(client.get_min_topup(), 1_000000i128);

    let new_min = 10_000000i128;
    client.set_min_topup(&ctx.admin, &new_min);
    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_set_min_topup_unauthorized() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let non_admin = Address::generate(&ctx.env);

    let result = client.try_set_min_topup(&non_admin, &5_000000i128);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_invalid_min_topup_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let admin = Address::generate(&env);

    assert_eq!(client.try_init(&token_address, &admin, &0i128), Err(Ok(Error::InvalidAmount)));
    client.init(&token_address, &admin, &1_000000i128);
    assert_eq!(client.try_set_min_topup(&admin, &0i128), Err(Ok(Error::InvalidAmount)));
}
