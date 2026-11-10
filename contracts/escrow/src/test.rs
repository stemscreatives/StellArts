//! Comprehensive unit test suite for the Escrow contract happy path
//! This module verifies the complete workflow: initialize engagement → mock token → mint tokens → deposit → release
//! Each test step includes assertions on token balances and state transitions
#[cfg(test)]
mod happy_path_tests {
    use crate::{DataKey, Escrow, EscrowContract, EscrowContractClient, Status};
    use soroban_sdk::testutils::{Address as AddressTestUtils, Events, Ledger};
    use soroban_sdk::{token, Address, Env};

    /// Test context holding common test objects
    struct TestContext {
        env: Env,
        contract_id: Address,
        token_address: Address,
        default_arbitrator: Address,
        client_contract: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_contract_client: token::StellarAssetClient<'static>,
    }

    impl TestContext {
        /// Initialize a test context with contract and token
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let token_admin = Address::generate(&env);
            let token_contract = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = token_contract.address();
            let default_arbitrator = Address::generate(&env);

            let client_contract = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_contract_client = token::StellarAssetClient::new(&env, &token_address);

            TestContext {
                env,
                contract_id,
                token_address,
                default_arbitrator,
                client_contract,
                token_client,
                token_contract_client,
            }
        }

        /// Get escrow from storage
        fn get_escrow(&self, engagement_id: u64) -> Escrow {
            self.env.as_contract(&self.contract_id, || {
                self.env
                    .storage()
                    .persistent()
                    .get(&DataKey::Escrow(engagement_id))
                    .expect("Escrow should exist")
            })
        }

        /// Initialize an engagement with the default arbitrator and token
        fn initialize_engagement(
            &self,
            client: &Address,
            artisan: &Address,
            material_amount: i128,
            labor_amount: i128,
        ) -> u64 {
            self.initialize_engagement_with_arbitrator(
                client,
                artisan,
                &self.default_arbitrator.clone(),
                material_amount,
                labor_amount,
            )
        }

        /// Initialize an engagement with a specific arbitrator and the default token
        fn initialize_engagement_with_arbitrator(
            &self,
            client: &Address,
            artisan: &Address,
            arbitrator: &Address,
            material_amount: i128,
            labor_amount: i128,
        ) -> u64 {
            let deadline = self.env.ledger().timestamp() + 86400;
            self.client_contract.initialize(
                client,
                artisan,
                arbitrator,
                &self.token_address,
                &material_amount,
                &labor_amount,
                &deadline,
                &soroban_sdk::vec![&self.env],
                &0u32,
                &soroban_sdk::vec![&self.env],
            )
        }

        /// Mint tokens to an address
        fn mint_tokens(&self, address: &Address, amount: i128) {
            self.token_contract_client.mint(address, &amount);
        }

        /// Deposit funds into an escrow
        fn deposit_funds(&self, engagement_id: u64) {
            self.client_contract
                .deposit(&engagement_id, &self.token_address);
        }

        /// Release funds from an escrow
        fn release_funds(&self, engagement_id: u64) {
            self.client_contract
                .release(&engagement_id, &self.token_address);
        }

        /// Full workflow: initialize, mint, deposit
        fn full_deposit_workflow(
            &self,
            client: &Address,
            artisan: &Address,
            material_amount: i128,
            labor_amount: i128,
        ) -> u64 {
            let engagement_id =
                self.initialize_engagement(client, artisan, material_amount, labor_amount);
            self.mint_tokens(client, material_amount + labor_amount);
            self.deposit_funds(engagement_id);
            engagement_id
        }

        /// Full workflow: initialize, mint, deposit, release
        fn full_workflow(
            &self,
            client: &Address,
            artisan: &Address,
            material_amount: i128,
            labor_amount: i128,
        ) -> u64 {
            let engagement_id =
                self.full_deposit_workflow(client, artisan, material_amount, labor_amount);
            self.release_funds(engagement_id);
            engagement_id
        }
    }

    /// Helper to create test addresses
    fn create_addresses(env: &Env) -> (Address, Address) {
        (Address::generate(env), Address::generate(env))
    }

    /// Test 1: Initialize Engagement
    /// Verifies that an engagement can be initialized with correct state
    #[test]
    fn test_happy_path_initialize_engagement() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let engagement_id = ctx.initialize_engagement(&client, &artisan, amount, labor_amount);

        assert_eq!(engagement_id, 1, "First engagement should have ID 1");

        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.client, client);
        assert_eq!(escrow.artisan, artisan);
        assert_eq!(escrow.material_amount, amount);
        assert_eq!(escrow.labor_amount, labor_amount);
        assert_eq!(escrow.status, Status::Pending);
    }

    /// Test 2: Mock Token Contract and Mint Tokens
    /// Verifies that tokens can be minted to the client's address
    #[test]
    fn test_happy_path_mint_tokens_to_client() {
        let ctx = TestContext::new();
        let client = Address::generate(&ctx.env);
        let initial_mint: i128 = 10000;

        ctx.mint_tokens(&client, initial_mint);

        let client_balance = ctx.token_client.balance(&client);
        assert_eq!(
            client_balance, initial_mint,
            "Client should have exactly minted amount"
        );

        let contract_balance = ctx.token_client.balance(&ctx.contract_id);
        assert_eq!(
            contract_balance, 0,
            "Contract should have no tokens before deposit"
        );
    }

    /// Test 3: Client Deposit into Escrow
    /// Verifies that client can deposit funds and escrow status transitions to Funded
    #[test]
    fn test_happy_path_client_deposit() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let escrow_amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let engagement_id =
            ctx.initialize_engagement(&client, &artisan, escrow_amount, labor_amount);
        ctx.mint_tokens(&client, 10000);

        let initial_client_balance = ctx.token_client.balance(&client);
        let initial_contract_balance = ctx.token_client.balance(&ctx.contract_id);

        ctx.deposit_funds(engagement_id);

        assert_eq!(
            ctx.token_client.balance(&client),
            initial_client_balance - escrow_amount,
            "Client balance should decrease by escrow amount"
        );
        assert_eq!(
            ctx.token_client.balance(&ctx.contract_id),
            initial_contract_balance + escrow_amount,
            "Contract balance should increase by escrow amount"
        );

        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Funded);
        assert_eq!(escrow.material_amount, escrow_amount);
    }

    /// Test 4: Release Funds to Artisan
    /// Verifies that client can release funds and artisan receives them
    #[test]
    fn test_happy_path_release_funds_to_artisan() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let escrow_amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let engagement_id =
            ctx.full_deposit_workflow(&client, &artisan, escrow_amount, labor_amount);

        let artisan_balance_before = ctx.token_client.balance(&artisan);
        ctx.release_funds(engagement_id);

        assert_eq!(
            ctx.token_client.balance(&artisan),
            artisan_balance_before + escrow_amount,
            "Artisan should receive the escrow amount"
        );
        assert_eq!(
            ctx.token_client.balance(&ctx.contract_id),
            0,
            "Contract should have no tokens after release"
        );

        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Released);
    }

    /// Test 5: Complete Happy Path - Full Workflow
    /// Verifies the entire workflow from initialization through release with comprehensive balance checks
    #[test]
    fn test_happy_path_complete_workflow() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let engagement_id = ctx.full_workflow(&client, &artisan, amount, labor_amount);

        // Verify final balances
        assert_eq!(
            ctx.token_client.balance(&client),
            0,
            "Client has no balance after depositing exact amount and releasing"
        );
        assert_eq!(
            ctx.token_client.balance(&ctx.contract_id),
            0,
            "Contract has no tokens after release"
        );
        assert_eq!(
            ctx.token_client.balance(&artisan),
            amount,
            "Artisan received the escrowed amount"
        );

        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Released);

        // Verify token conservation
        let total = ctx.token_client.balance(&client)
            + ctx.token_client.balance(&ctx.contract_id)
            + ctx.token_client.balance(&artisan);
        assert_eq!(total, amount, "Total tokens conserved");
    }

    /// Test 6: Multiple Engagements
    /// Verifies that multiple engagements can be managed independently
    #[test]
    fn test_happy_path_multiple_engagements() {
        let ctx = TestContext::new();

        // First engagement
        let (client1, artisan1) = create_addresses(&ctx.env);
        let amount1 = 1000i128;
        let id1 = ctx.full_deposit_workflow(&client1, &artisan1, amount1, 0);

        // Second engagement
        let (client2, artisan2) = create_addresses(&ctx.env);
        let amount2 = 2000i128;
        let id2 = ctx.full_deposit_workflow(&client2, &artisan2, amount2, 0);

        // Verify both are funded
        assert_eq!(
            ctx.token_client.balance(&ctx.contract_id),
            amount1 + amount2,
            "Contract holds both amounts"
        );

        // Release both
        ctx.release_funds(id1);
        ctx.release_funds(id2);

        // Verify all released
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }

    /// Test 7: Large Amount Handling
    /// Verifies that large amounts are handled correctly without overflow
    #[test]
    fn test_happy_path_large_amounts() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let large_amount: i128 = 1_000_000_000_000;

        ctx.full_workflow(&client, &artisan, large_amount, 0);

        assert_eq!(ctx.token_client.balance(&artisan), large_amount);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }

    /// Test 8: Exact Amount Matching
    /// Verifies behavior when client deposits exact amount with no extra tokens
    #[test]
    fn test_happy_path_exact_amount_deposit() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let engagement_id = ctx.initialize_engagement(&client, &artisan, amount, labor_amount);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        assert_eq!(ctx.token_client.balance(&client), 0);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), amount);

        ctx.release_funds(engagement_id);
        assert_eq!(ctx.token_client.balance(&artisan), amount);
    }

    /// Test 9: Sequential Operations
    /// Verifies that multiple sequential engagements work correctly
    #[test]
    fn test_happy_path_sequential_operations() {
        let ctx = TestContext::new();

        for i in 1..=3 {
            let (client, artisan) = create_addresses(&ctx.env);
            let amount: i128 = 1000 * i as i128;

            let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);

            // After deposit, contract should hold the amount (no other engagements pending)
            assert_eq!(
                ctx.token_client.balance(&ctx.contract_id),
                amount,
                "Contract holds current engagement amount at iteration {i}",
            );

            ctx.release_funds(engagement_id);

            // After release, contract should be empty for this iteration
            assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
            assert_eq!(ctx.token_client.balance(&artisan), amount);
        }
    }

    /// Test 10: State Consistency
    /// Verifies that escrow state remains consistent through the happy path
    #[test]
    fn test_happy_path_state_consistency() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        // Initialize and check state
        let engagement_id = ctx.initialize_engagement(&client, &artisan, amount, labor_amount);
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Pending);
        assert_eq!(escrow.client, client);
        assert_eq!(escrow.artisan, artisan);

        // Deposit and check state
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Funded);
        assert_eq!(escrow.client, client);
        assert_eq!(escrow.material_amount, amount);

        // Release and check final state
        ctx.release_funds(engagement_id);
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Released);
        assert_eq!(escrow.client, client);
        assert_eq!(escrow.artisan, artisan);
    }

    /// Test 11: Deadline lifecycle – deposit before deadline, reclaim after expiry
    #[test]
    fn test_happy_path_deadline_lifecycle() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        // set a short deadline a few seconds in the future
        let now = ctx.env.ledger().timestamp();
        let deadline = now + 10;
        let engagement_id = ctx.client_contract.initialize(
            &client,
            &artisan,
            &ctx.default_arbitrator,
            &ctx.token_address,
            &amount,
            &labor_amount,
            &deadline,
            &soroban_sdk::vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );

        // fund and deposit before the deadline
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);
        assert_eq!(ctx.get_escrow(engagement_id).status, Status::Funded);

        // fast-forward ledger past the deadline + GRACE_PERIOD (86400 seconds)
        ctx.env.ledger().set_timestamp(deadline + 86400 + 1);

        // now reclaim should succeed and return funds to client
        let client_balance_before = ctx.token_client.balance(&client);
        ctx.client_contract
            .reclaim(&engagement_id, &ctx.token_address);
        let client_balance_after = ctx.token_client.balance(&client);
        assert_eq!(client_balance_after, client_balance_before + amount);

        // ensure an event from the escrow contract was emitted (token contract also emits events)
        let raw_events: soroban_sdk::Vec<(
            Address,
            soroban_sdk::Vec<soroban_sdk::Val>,
            soroban_sdk::Val,
        )> = ctx.env.events().all();
        let mut found = false;
        for (addr, _, _) in raw_events.into_iter() {
            if addr == ctx.contract_id {
                found = true;
                break;
            }
        }
        assert!(found, "expected at least one escrow contract event");

        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Refunded);
    }

    #[test]
    fn test_extend_deadline_client_proposes_artisan_confirms() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);
        let original_deadline = ctx.get_escrow(engagement_id).deadline;
        let new_deadline = original_deadline + 3600;

        ctx.client_contract
            .extend_deadline(&engagement_id, &client, &new_deadline);
        assert_eq!(ctx.get_escrow(engagement_id).deadline, original_deadline);

        ctx.client_contract
            .extend_deadline(&engagement_id, &artisan, &new_deadline);
        assert_eq!(ctx.get_escrow(engagement_id).deadline, new_deadline);
    }

    #[test]
    fn test_extend_deadline_artisan_proposes_client_confirms() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);
        let original_deadline = ctx.get_escrow(engagement_id).deadline;
        let new_deadline = original_deadline + 7200;

        ctx.client_contract
            .extend_deadline(&engagement_id, &artisan, &new_deadline);
        assert_eq!(ctx.get_escrow(engagement_id).deadline, original_deadline);

        ctx.client_contract
            .extend_deadline(&engagement_id, &client, &new_deadline);
        assert_eq!(ctx.get_escrow(engagement_id).deadline, new_deadline);
    }

    #[test]
    #[should_panic(expected = "Only client or artisan can approve deadline extension")]
    fn test_extend_deadline_unauthorized_caller_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let unauthorized = Address::generate(&ctx.env);
        let amount: i128 = 5000;
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);
        let new_deadline = ctx.get_escrow(engagement_id).deadline + 3600;

        ctx.client_contract
            .extend_deadline(&engagement_id, &unauthorized, &new_deadline);
    }

    #[test]
    #[should_panic(expected = "Same party cannot approve deadline extension twice")]
    fn test_extend_deadline_same_party_approves_twice_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);
        let new_deadline = ctx.get_escrow(engagement_id).deadline + 3600;

        ctx.client_contract
            .extend_deadline(&engagement_id, &client, &new_deadline);
        ctx.client_contract
            .extend_deadline(&engagement_id, &client, &new_deadline);
    }

    #[test]
    #[should_panic(expected = "Pending deadline extension does not match requested deadline")]
    fn test_extend_deadline_mismatched_deadline_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);
        let original_deadline = ctx.get_escrow(engagement_id).deadline;

        ctx.client_contract
            .extend_deadline(&engagement_id, &client, &(original_deadline + 3600));
        ctx.client_contract
            .extend_deadline(&engagement_id, &artisan, &(original_deadline + 7200));
    }

    #[test]
    #[should_panic(expected = "New deadline must be greater than current deadline")]
    fn test_extend_deadline_non_increasing_deadline_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);
        let current_deadline = ctx.get_escrow(engagement_id).deadline;

        ctx.client_contract
            .extend_deadline(&engagement_id, &client, &current_deadline);
    }

    /// Test 12: Dispute - client initiates dispute on funded escrow
    #[test]
    fn test_dispute_from_client() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize, mint, and deposit
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);

        // Verify initial state is Funded
        assert_eq!(ctx.get_escrow(engagement_id).status, Status::Funded);

        // Client initiates dispute
        ctx.client_contract.dispute(&engagement_id, &client);

        // Verify status transitioned to Disputed
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Disputed);
    }

    /// Test 13: Dispute - artisan initiates dispute on funded escrow
    #[test]
    fn test_dispute_from_artisan() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize, mint, and deposit
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);

        // Verify initial state is Funded
        assert_eq!(ctx.get_escrow(engagement_id).status, Status::Funded);

        // Artisan initiates dispute
        ctx.client_contract.dispute(&engagement_id, &artisan);

        // Verify status transitioned to Disputed
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Disputed);
    }

    /// Test 14: Dispute from wrong state - attempt dispute on Pending escrow should fail
    #[test]
    #[should_panic(expected = "Escrow must be Funded or InProgress to initiate a dispute")]
    fn test_dispute_from_pending_state_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize but don't deposit (escrow remains Pending)
        let engagement_id = ctx.initialize_engagement(&client, &artisan, amount, 0);

        // Attempt to dispute Pending escrow should fail
        ctx.client_contract.dispute(&engagement_id, &client);
    }

    /// Test 15: Double dispute rejection - second dispute on already disputed escrow should fail
    #[test]
    #[should_panic(expected = "Escrow must be Funded or InProgress to initiate a dispute")]
    fn test_double_dispute_rejection() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize, mint, and deposit
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);

        // First dispute succeeds
        ctx.client_contract.dispute(&engagement_id, &client);
        assert_eq!(ctx.get_escrow(engagement_id).status, Status::Disputed);

        // Second dispute should fail
        ctx.client_contract.dispute(&engagement_id, &client);
    }

    /// Test 16: Unauthorized dispute - third party cannot initiate dispute
    #[test]
    #[should_panic(expected = "Only client or artisan can initiate a dispute")]
    fn test_unauthorized_dispute_attempt() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize, mint, and deposit
        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);

        // Third party attempts to dispute
        let unauthorized = Address::generate(&ctx.env);
        ctx.client_contract.dispute(&engagement_id, &unauthorized);
    }

    /// Test 17: Resolve dispute - arbitrator resolves dispute fully in favor of client (refund)
    #[test]
    fn test_resolve_dispute_full_to_client() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Initiate dispute
        ctx.client_contract.dispute(&engagement_id, &client);
        assert_eq!(ctx.get_escrow(engagement_id).status, Status::Disputed);

        // Get client balance before resolution
        let client_balance_before = ctx.token_client.balance(&client);

        // Arbitrator resolves: 100% to client, 0% to artisan
        ctx.client_contract
            .resolve_dispute(&engagement_id, &amount, &0, &ctx.token_address);

        // Verify funds transferred to client
        let client_balance_after = ctx.token_client.balance(&client);
        assert_eq!(client_balance_after, client_balance_before + amount);

        // Verify status transitioned to Refunded
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Refunded);
    }

    /// Test 18: Resolve dispute - arbitrator resolves dispute fully in favor of artisan (release)
    #[test]
    fn test_resolve_dispute_full_to_artisan() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Initiate dispute
        ctx.client_contract.dispute(&engagement_id, &artisan);
        assert_eq!(ctx.get_escrow(engagement_id).status, Status::Disputed);

        // Get artisan balance before resolution
        let artisan_balance_before = ctx.token_client.balance(&artisan);

        // Arbitrator resolves: 0% to client, 100% to artisan
        ctx.client_contract
            .resolve_dispute(&engagement_id, &0, &amount, &ctx.token_address);

        // Verify funds transferred to artisan
        let artisan_balance_after = ctx.token_client.balance(&artisan);
        assert_eq!(artisan_balance_after, artisan_balance_before + amount);

        // Verify status transitioned to Released
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Released);
    }

    /// Test 19: Unauthorized resolve_dispute - non-arbitrator cannot resolve disputes
    #[test]
    #[should_panic]
    fn test_unauthorized_resolve_dispute_attempt() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Initiate dispute
        ctx.client_contract.dispute(&engagement_id, &client);

        // Disable global mock auth and ensure only real arbitrator can call
        ctx.env.set_auths(&[]);

        // Unauthorized user attempts to resolve dispute
        ctx.client_contract
            .resolve_dispute(&engagement_id, &amount, &0, &ctx.token_address);
    }

    /// Test 20: Resolve dispute - distribution amounts that don't sum to escrow amount should fail
    #[test]
    #[should_panic(expected = "Distribution amounts must equal the remaining escrowed amount")]
    fn test_resolve_dispute_invalid_amounts() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Initiate dispute
        ctx.client_contract.dispute(&engagement_id, &client);

        // Try to distribute incorrect total (3000 + 3000 != 5000)
        ctx.client_contract
            .resolve_dispute(&engagement_id, &3000, &3000, &ctx.token_address);
    }

    /// Test 21: Resolve dispute from non-disputed state should fail
    #[test]
    #[should_panic(expected = "Escrow must be in Disputed status to resolve")]
    fn test_resolve_dispute_from_non_disputed_state_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit (but don't dispute)
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Arbitrator tries to resolve non-disputed escrow
        ctx.client_contract
            .resolve_dispute(&engagement_id, &amount, &0, &ctx.token_address);
    }

    /// Test 22: Resolve dispute with split distribution (e.g. 60/40)
    #[test]
    fn test_resolve_dispute_split_distribution() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Initiate dispute
        ctx.client_contract.dispute(&engagement_id, &client);
        assert_eq!(ctx.get_escrow(engagement_id).status, Status::Disputed);

        // Record balances before
        let client_balance_before = ctx.token_client.balance(&client);
        let artisan_balance_before = ctx.token_client.balance(&artisan);

        // Arbitrator resolves: 60% to client (3000), 40% to artisan (2000)
        let client_share: i128 = 3000;
        let artisan_share: i128 = 2000;
        ctx.client_contract.resolve_dispute(
            &engagement_id,
            &client_share,
            &artisan_share,
            &ctx.token_address,
        );

        // Verify funds distributed correctly
        assert_eq!(
            ctx.token_client.balance(&client),
            client_balance_before + client_share,
            "Client should receive their share"
        );
        assert_eq!(
            ctx.token_client.balance(&artisan),
            artisan_balance_before + artisan_share,
            "Artisan should receive their share"
        );
        assert_eq!(
            ctx.token_client.balance(&ctx.contract_id),
            0,
            "Contract should have no tokens after resolution"
        );

        // Verify status is Resolved (split)
        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Resolved);
    }

    /// Test 23: Resolve dispute with negative amount should fail
    #[test]
    #[should_panic(expected = "Distribution amounts must be non-negative")]
    fn test_resolve_dispute_negative_amount_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Initiate dispute
        ctx.client_contract.dispute(&engagement_id, &client);

        // Try negative client_amount
        ctx.client_contract
            .resolve_dispute(&engagement_id, &(-1000), &6000, &ctx.token_address);
    }

    /// Test 24: Resolve dispute emits correct event with distribution details
    #[test]
    fn test_resolve_dispute_event_emitted() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        // Setup: Initialize with per-escrow arbitrator, mint, and deposit
        let engagement_id =
            ctx.initialize_engagement_with_arbitrator(&client, &artisan, &arbitrator, amount, 0);
        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Initiate dispute
        ctx.client_contract.dispute(&engagement_id, &client);

        // Resolve with split: 2000 to client, 3000 to artisan
        ctx.client_contract
            .resolve_dispute(&engagement_id, &2000, &3000, &ctx.token_address);

        // Verify at least one event from the escrow contract was emitted
        let raw_events: soroban_sdk::Vec<(
            Address,
            soroban_sdk::Vec<soroban_sdk::Val>,
            soroban_sdk::Val,
        )> = ctx.env.events().all();
        let mut found = false;
        for (addr, _, _) in raw_events.into_iter() {
            if addr == ctx.contract_id {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "expected a DisputeResolvedEvent from the escrow contract"
        );
    }

    /// Test 26: Reclaim fails during grace period without mutual approval
    #[test]
    #[should_panic(
        expected = "Grace period has not passed; both parties must approve early reclaim"
    )]
    fn test_reclaim_fails_during_grace_period() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let now = ctx.env.ledger().timestamp();
        let deadline = now + 10;
        let engagement_id = ctx.client_contract.initialize(
            &client,
            &artisan,
            &ctx.default_arbitrator,
            &ctx.token_address,
            &amount,
            &labor_amount,
            &deadline,
            &soroban_sdk::vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );

        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Fast-forward past deadline but within grace period (deadline + 1 hour)
        ctx.env.ledger().set_timestamp(deadline + 3600);

        // Should panic: still within 24h grace period
        ctx.client_contract
            .reclaim(&engagement_id, &ctx.token_address);
    }

    /// Test 27: Reclaim succeeds after deadline + grace period
    #[test]
    fn test_reclaim_succeeds_after_grace_period() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let now = ctx.env.ledger().timestamp();
        let deadline = now + 10;
        let engagement_id = ctx.client_contract.initialize(
            &client,
            &artisan,
            &ctx.default_arbitrator,
            &ctx.token_address,
            &amount,
            &labor_amount,
            &deadline,
            &soroban_sdk::vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );

        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Fast-forward past deadline + GRACE_PERIOD (86400 seconds)
        ctx.env.ledger().set_timestamp(deadline + 86400 + 1);

        let client_balance_before = ctx.token_client.balance(&client);
        ctx.client_contract
            .reclaim(&engagement_id, &ctx.token_address);
        let client_balance_after = ctx.token_client.balance(&client);
        assert_eq!(client_balance_after, client_balance_before + amount);

        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Refunded);
    }

    /// Test 28: Early reclaim with mutual approval during grace period
    #[test]
    fn test_early_reclaim_with_mutual_approval() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;
        let labor_amount: i128 = 0;

        let now = ctx.env.ledger().timestamp();
        let deadline = now + 10;
        let engagement_id = ctx.client_contract.initialize(
            &client,
            &artisan,
            &ctx.default_arbitrator,
            &ctx.token_address,
            &amount,
            &labor_amount,
            &deadline,
            &soroban_sdk::vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );

        ctx.mint_tokens(&client, amount);
        ctx.deposit_funds(engagement_id);

        // Fast-forward past deadline but within grace period
        ctx.env.ledger().set_timestamp(deadline + 100);

        // Both parties approve early reclaim
        ctx.client_contract
            .approve_early_reclaim(&engagement_id, &client);
        ctx.client_contract
            .approve_early_reclaim(&engagement_id, &artisan);

        // Now reclaim should succeed during grace period
        let client_balance_before = ctx.token_client.balance(&client);
        ctx.client_contract
            .reclaim(&engagement_id, &ctx.token_address);
        let client_balance_after = ctx.token_client.balance(&client);
        assert_eq!(client_balance_after, client_balance_before + amount);

        let escrow = ctx.get_escrow(engagement_id);
        assert_eq!(escrow.status, Status::Refunded);
    }

    /// Test 29: Unauthorized early reclaim approval fails
    #[test]
    #[should_panic(expected = "Only client or artisan can approve early reclaim")]
    fn test_early_reclaim_unauthorized_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let unauthorized = Address::generate(&ctx.env);
        let amount: i128 = 5000;

        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);

        // Third party attempts to approve early reclaim
        ctx.client_contract
            .approve_early_reclaim(&engagement_id, &unauthorized);
    }

    /// Test 30: Same party cannot approve early reclaim twice
    #[test]
    #[should_panic(expected = "Same party cannot approve early reclaim twice")]
    fn test_early_reclaim_same_party_twice_fails() {
        let ctx = TestContext::new();
        let (client, artisan) = create_addresses(&ctx.env);
        let amount: i128 = 5000;

        let engagement_id = ctx.full_deposit_workflow(&client, &artisan, amount, 0);

        // Client approves twice
        ctx.client_contract
            .approve_early_reclaim(&engagement_id, &client);
        ctx.client_contract
            .approve_early_reclaim(&engagement_id, &client);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #217 – TTL snapshot tests using jump_ledgers
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod ttl_snapshot_tests {
    use crate::{DataKey, EscrowContract, EscrowContractClient, Status};
    use soroban_sdk::testutils::{Address as AddressTestUtils, Ledger, LedgerInfo};
    use soroban_sdk::{token, Address, Env};

    const DAY_LEDGERS: u32 = 17_280; // ~1 day at 5 s/ledger
    #[allow(dead_code)]
    const LEDGERS_PER_SECOND: u32 = 1; // 1 ledger ≈ 5 s; we use 1 for simplicity
    #[allow(dead_code)]
    const ESCROW_TTL: u32 = 1_036_800; // ~60 days

    struct TtlCtx {
        env: Env,
        contract_id: Address,
        token_address: Address,
        client: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_asset_client: token::StellarAssetClient<'static>,
    }

    impl TtlCtx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let token_admin = Address::generate(&env);
            let token_contract = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = token_contract.address();
            let client = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_asset_client = token::StellarAssetClient::new(&env, &token_address);
            TtlCtx {
                env,
                contract_id,
                token_address,
                client,
                token_client,
                token_asset_client,
            }
        }

        /// Advance the ledger by `ledgers` ledgers, updating both sequence and timestamp.
        /// Extends both contract instance TTLs before the jump so they are not archived.
        fn jump_ledgers(&self, ledgers: u32) {
            let ttl = ledgers + 1_036_800;
            // Extend escrow contract instance TTL before advancing.
            self.env.as_contract(&self.contract_id, || {
                self.env.storage().instance().extend_ttl(ttl, ttl);
            });
            // Extend token contract instance TTL before advancing.
            self.env.as_contract(&self.token_address, || {
                self.env.storage().instance().extend_ttl(ttl, ttl);
            });
            let current = self.env.ledger().get();
            self.env.ledger().set(LedgerInfo {
                sequence_number: current.sequence_number + ledgers,
                timestamp: current.timestamp + (ledgers as u64) * 5,
                ..current
            });
        }

        fn initialize_and_fund(
            &self,
            material_amount: i128,
            labor_amount: i128,
        ) -> (u64, Address, Address) {
            let client_addr = Address::generate(&self.env);
            let artisan_addr = Address::generate(&self.env);
            let arbitrator = Address::generate(&self.env);
            let deadline = self.env.ledger().timestamp() + 86400 * 90; // 90 days
            let id = self.client.initialize(
                &client_addr,
                &artisan_addr,
                &arbitrator,
                &self.token_address,
                &material_amount,
                &labor_amount,
                &deadline,
                &soroban_sdk::vec![&self.env],
                &0u32,
                &soroban_sdk::vec![&self.env],
            );
            self.token_asset_client
                .mint(&client_addr, &(material_amount + labor_amount));
            self.client.deposit(&id, &self.token_address);
            (id, client_addr, artisan_addr)
        }
    }

    /// TTL-1: Record persists after simulating 30+ days of ledger advancement.
    #[test]
    fn test_ttl_record_persists_after_30_days() {
        let ctx = TtlCtx::new();
        let (id, _, _) = ctx.initialize_and_fund(1_000, 0);

        // Advance ~30 days (30 * DAY_LEDGERS)
        ctx.jump_ledgers(30 * DAY_LEDGERS);

        // Escrow record must still be readable
        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .expect("Escrow should still exist after 30 days")
        });
        assert_eq!(escrow.status, Status::Funded);
    }

    /// TTL-2: TTL extends on deposit state transition.
    #[test]
    fn test_ttl_extends_on_deposit() {
        let ctx = TtlCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 86400 * 90;
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &1_000i128,
            &0i128,
            &deadline,
            &soroban_sdk::vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );

        // Advance a few days before depositing
        ctx.jump_ledgers(5 * DAY_LEDGERS);

        ctx.token_asset_client.mint(&client_addr, &1_000i128);
        ctx.client.deposit(&id, &ctx.token_address);

        // After deposit the TTL should have been refreshed; advance another 30 days
        ctx.jump_ledgers(30 * DAY_LEDGERS);

        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .expect("Escrow should persist after TTL extension on deposit")
        });
        assert_eq!(escrow.status, Status::Funded);
    }

    /// TTL-3: TTL extends on release state transition.
    #[test]
    fn test_ttl_extends_on_release() {
        let ctx = TtlCtx::new();
        let (id, _, artisan_addr) = ctx.initialize_and_fund(2_000, 0);

        // Advance 10 days, then release
        ctx.jump_ledgers(10 * DAY_LEDGERS);
        ctx.client.release(&id, &ctx.token_address);

        // Advance another 30 days – record should still be readable (Released status)
        ctx.jump_ledgers(30 * DAY_LEDGERS);

        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .expect("Released escrow should persist after TTL extension")
        });
        assert_eq!(escrow.status, Status::Released);
        assert_eq!(ctx.token_client.balance(&artisan_addr), 2_000);
    }

    /// TTL-4: TTL extends on reclaim (Refunded) state transition.
    #[test]
    fn test_ttl_extends_on_reclaim() {
        let ctx = TtlCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 10;
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &500i128,
            &0i128,
            &deadline,
            &soroban_sdk::vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &500i128);
        ctx.client.deposit(&id, &ctx.token_address);

        // Jump past deadline + grace period
        ctx.jump_ledgers(20 * DAY_LEDGERS);
        ctx.client.reclaim(&id, &ctx.token_address);

        // Advance another 30 days – record should still be readable
        ctx.jump_ledgers(30 * DAY_LEDGERS);

        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .expect("Refunded escrow should persist after TTL extension")
        });
        assert_eq!(escrow.status, Status::Refunded);
    }

    /// TTL-5: TTL extends on dispute state transition.
    #[test]
    fn test_ttl_extends_on_dispute() {
        let ctx = TtlCtx::new();
        let (id, client_addr, _) = ctx.initialize_and_fund(3_000, 0);

        ctx.jump_ledgers(5 * DAY_LEDGERS);
        ctx.client.dispute(&id, &client_addr);

        ctx.jump_ledgers(30 * DAY_LEDGERS);

        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .expect("Disputed escrow should persist after TTL extension")
        });
        assert_eq!(escrow.status, Status::Disputed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #211 – Multi-sig support tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod multisig_tests {
    use crate::{DataKey, EscrowContract, EscrowContractClient, Status};
    use soroban_sdk::testutils::Address as AddressTestUtils;
    use soroban_sdk::{token, vec, Address, Env};

    struct MsCtx {
        env: Env,
        contract_id: Address,
        token_address: Address,
        client: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_asset_client: token::StellarAssetClient<'static>,
    }

    impl MsCtx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let token_admin = Address::generate(&env);
            let tc = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = tc.address();
            let client = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_asset_client = token::StellarAssetClient::new(&env, &token_address);
            MsCtx {
                env,
                contract_id,
                token_address,
                client,
                token_client,
                token_asset_client,
            }
        }
    }

    /// MS-1: Single-sig escrow (no multisig) still works as before.
    #[test]
    fn test_single_sig_release_unchanged() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount = 1_000i128;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &vec![&ctx.env], // empty → no multisig
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);
        ctx.client.release(&id, &ctx.token_address);

        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .unwrap()
        });
        assert_eq!(escrow.status, Status::Released);
        assert_eq!(ctx.token_client.balance(&artisan_addr), amount);
    }

    /// MS-2: Multi-sig release succeeds when threshold is met (2-of-2).
    #[test]
    fn test_multisig_release_succeeds_when_threshold_met() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let signer1 = Address::generate(&ctx.env);
        let signer2 = Address::generate(&ctx.env);
        let amount = 5_000i128;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let signers = vec![&ctx.env, signer1.clone(), signer2.clone()];
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &signers,
            &2u32, // 2-of-2
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);

        // Both signers approve
        ctx.client.multisig_approve(&id, &signer1);
        ctx.client.multisig_approve(&id, &signer2);

        // Release should now succeed
        ctx.client.release(&id, &ctx.token_address);

        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .unwrap()
        });
        assert_eq!(escrow.status, Status::Released);
        assert_eq!(ctx.token_client.balance(&artisan_addr), amount);
    }

    /// MS-3: Release fails when multi-sig threshold is not yet met.
    #[test]
    #[should_panic(expected = "Multi-sig threshold not met")]
    fn test_multisig_release_fails_without_approvals() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let signer1 = Address::generate(&ctx.env);
        let signer2 = Address::generate(&ctx.env);
        let amount = 5_000i128;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let signers = vec![&ctx.env, signer1.clone(), signer2.clone()];
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &signers,
            &2u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);

        // Only one signer approves – threshold not met
        ctx.client.multisig_approve(&id, &signer1);

        // Should panic
        ctx.client.release(&id, &ctx.token_address);
    }

    /// MS-4: 1-of-2 threshold – release succeeds after only one approval.
    #[test]
    fn test_multisig_1_of_2_threshold() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let signer1 = Address::generate(&ctx.env);
        let signer2 = Address::generate(&ctx.env);
        let amount = 2_000i128;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let signers = vec![&ctx.env, signer1.clone(), signer2.clone()];
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &signers,
            &1u32, // 1-of-2
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);

        // Only signer1 approves
        ctx.client.multisig_approve(&id, &signer1);
        ctx.client.release(&id, &ctx.token_address);

        assert_eq!(ctx.token_client.balance(&artisan_addr), amount);
    }

    /// MS-5: Unauthorized signer cannot approve.
    #[test]
    #[should_panic(expected = "Signer is not in the multi-sig required signers list")]
    fn test_multisig_unauthorized_signer_rejected() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let signer1 = Address::generate(&ctx.env);
        let unauthorized = Address::generate(&ctx.env);
        let amount = 1_000i128;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let signers = vec![&ctx.env, signer1.clone()];
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &signers,
            &1u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);

        // Unauthorized address tries to approve
        ctx.client.multisig_approve(&id, &unauthorized);
    }

    /// MS-6: Same signer cannot approve twice.
    #[test]
    #[should_panic(expected = "Signer has already approved this escrow")]
    fn test_multisig_duplicate_approval_rejected() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let signer1 = Address::generate(&ctx.env);
        let amount = 1_000i128;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let signers = vec![&ctx.env, signer1.clone()];
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &signers,
            &1u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);

        ctx.client.multisig_approve(&id, &signer1);
        ctx.client.multisig_approve(&id, &signer1); // duplicate – should panic
    }

    /// MS-7: Invalid threshold (0) is rejected at initialization.
    #[test]
    #[should_panic(expected = "multisig_threshold must be between 1 and the number of signers")]
    fn test_multisig_zero_threshold_rejected() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let signer1 = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let signers = vec![&ctx.env, signer1.clone()];
        ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &1_000i128,
            &0i128,
            &deadline,
            &signers,
            &0u32, // invalid
            &soroban_sdk::vec![&ctx.env],
        );
    }

    /// MS-8: Threshold exceeding signer count is rejected.
    #[test]
    #[should_panic(expected = "multisig_threshold must be between 1 and the number of signers")]
    fn test_multisig_threshold_exceeds_signers_rejected() {
        let ctx = MsCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let signer1 = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let signers = vec![&ctx.env, signer1.clone()]; // 1 signer
        ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &1_000i128,
            &0i128,
            &deadline,
            &signers,
            &3u32, // threshold > signers count
            &soroban_sdk::vec![&ctx.env],
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #212 – cleanup_expired tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod cleanup_tests {
    use crate::{DataKey, EscrowContract, EscrowContractClient};
    use soroban_sdk::testutils::{Address as AddressTestUtils, Events, Ledger};
    use soroban_sdk::{token, vec, Address, Env, Vec};

    struct CleanCtx {
        env: Env,
        contract_id: Address,
        token_address: Address,
        client: EscrowContractClient<'static>,
        #[allow(dead_code)]
        token_client: token::Client<'static>,
        token_asset_client: token::StellarAssetClient<'static>,
    }

    impl CleanCtx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let token_admin = Address::generate(&env);
            let tc = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = tc.address();
            let client = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_asset_client = token::StellarAssetClient::new(&env, &token_address);
            CleanCtx {
                env,
                contract_id,
                token_address,
                client,
                token_client,
                token_asset_client,
            }
        }

        fn create_released_escrow(&self) -> (u64, Address) {
            let client_addr = Address::generate(&self.env);
            let artisan_addr = Address::generate(&self.env);
            let arbitrator = Address::generate(&self.env);
            let amount = 1_000i128;
            let deadline = self.env.ledger().timestamp() + 86400;
            let id = self.client.initialize(
                &client_addr,
                &artisan_addr,
                &arbitrator,
                &self.token_address,
                &amount,
                &0i128,
                &deadline,
                &vec![&self.env],
                &0u32,
                &soroban_sdk::vec![&self.env],
            );
            self.token_asset_client.mint(&client_addr, &amount);
            self.client.deposit(&id, &self.token_address);
            self.client.release(&id, &self.token_address);
            (id, client_addr)
        }

        fn create_refunded_escrow(&self) -> (u64, Address) {
            let client_addr = Address::generate(&self.env);
            let artisan_addr = Address::generate(&self.env);
            let arbitrator = Address::generate(&self.env);
            let amount = 500i128;
            let deadline = self.env.ledger().timestamp() + 10;
            let id = self.client.initialize(
                &client_addr,
                &artisan_addr,
                &arbitrator,
                &self.token_address,
                &amount,
                &0i128,
                &deadline,
                &vec![&self.env],
                &0u32,
                &soroban_sdk::vec![&self.env],
            );
            self.token_asset_client.mint(&client_addr, &amount);
            self.client.deposit(&id, &self.token_address);
            // Jump past deadline + grace period
            let current = self.env.ledger().get();
            self.env.ledger().set(soroban_sdk::testutils::LedgerInfo {
                timestamp: deadline + 86400 + 1,
                ..current
            });
            self.client.reclaim(&id, &self.token_address);
            (id, client_addr)
        }
    }

    /// CL-1: cleanup_expired removes a Released escrow from storage.
    #[test]
    fn test_cleanup_removes_released_escrow() {
        let ctx = CleanCtx::new();
        let (id, _client_addr) = ctx.create_released_escrow();

        // Verify it exists
        assert!(ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env.storage().persistent().has(&DataKey::Escrow(id))
        }));

        let ids: Vec<u64> = vec![&ctx.env, id];
        ctx.client.cleanup_expired(&ids);

        // Verify it was removed
        assert!(!ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env.storage().persistent().has(&DataKey::Escrow(id))
        }));
    }

    /// CL-2: cleanup_expired removes a Refunded escrow from storage.
    #[test]
    fn test_cleanup_removes_refunded_escrow() {
        let ctx = CleanCtx::new();
        let (id, _) = ctx.create_refunded_escrow();

        assert!(ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env.storage().persistent().has(&DataKey::Escrow(id))
        }));

        let ids: Vec<u64> = vec![&ctx.env, id];
        ctx.client.cleanup_expired(&ids);

        assert!(!ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env.storage().persistent().has(&DataKey::Escrow(id))
        }));
    }

    /// CL-3: cleanup_expired emits a cleanup event for each removed escrow.
    #[test]
    fn test_cleanup_emits_event() {
        let ctx = CleanCtx::new();
        let (id, _) = ctx.create_released_escrow();

        let ids: Vec<u64> = vec![&ctx.env, id];
        ctx.client.cleanup_expired(&ids);

        let raw_events: soroban_sdk::Vec<(
            Address,
            soroban_sdk::Vec<soroban_sdk::Val>,
            soroban_sdk::Val,
        )> = ctx.env.events().all();
        let mut found = false;
        for (addr, _, _) in raw_events.into_iter() {
            if addr == ctx.contract_id {
                found = true;
                break;
            }
        }
        assert!(found, "expected a cleanup event from the escrow contract");
    }

    /// CL-4: cleanup_expired on a Funded escrow panics.
    #[test]
    #[should_panic(expected = "is not in a finalized state")]
    fn test_cleanup_funded_escrow_panics() {
        let ctx = CleanCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount = 1_000i128;
        let deadline = ctx.env.ledger().timestamp() + 86400;
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);

        let ids: Vec<u64> = vec![&ctx.env, id];
        ctx.client.cleanup_expired(&ids);
    }

    /// CL-5: cleanup_expired on multiple finalized escrows in one call.
    #[test]
    fn test_cleanup_multiple_escrows() {
        let ctx = CleanCtx::new();
        let (id1, _) = ctx.create_released_escrow();
        let (id2, _) = ctx.create_refunded_escrow();

        let ids: Vec<u64> = vec![&ctx.env, id1, id2];
        ctx.client.cleanup_expired(&ids);

        assert!(!ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env.storage().persistent().has(&DataKey::Escrow(id1))
        }));
        assert!(!ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env.storage().persistent().has(&DataKey::Escrow(id2))
        }));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue - Security Enhancement - Reentrancy tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod reentrancy_tests {
    use crate::{EscrowContract, EscrowContractClient};
    use soroban_sdk::{contract, contractimpl, testutils::Address as _, vec, Address, Env};

    // A mock token that attempts to reenter the escrow contract during a transfer
    #[contract]
    pub struct MaliciousToken;

    #[contractimpl]
    impl MaliciousToken {
        pub fn transfer(env: Env, _from: Address, _to: Address, _amount: i128) {
            let should_reenter: bool = env
                .storage()
                .instance()
                .get(&soroban_sdk::Symbol::new(&env, "reenter"))
                .unwrap_or(false);
            if should_reenter {
                // Prevent infinite loop by clearing the flag
                env.storage()
                    .instance()
                    .set(&soroban_sdk::Symbol::new(&env, "reenter"), &false);

                let escrow_addr: Address = env
                    .storage()
                    .instance()
                    .get(&soroban_sdk::Symbol::new(&env, "escrow"))
                    .unwrap();
                let engagement_id: u64 = env
                    .storage()
                    .instance()
                    .get(&soroban_sdk::Symbol::new(&env, "id"))
                    .unwrap();

                let client = EscrowContractClient::new(&env, &escrow_addr);

                // Reenter the release function
                client.release(&engagement_id, &env.current_contract_address());
            }
        }

        pub fn setup(env: Env, escrow: Address, id: u64) {
            env.storage()
                .instance()
                .set(&soroban_sdk::Symbol::new(&env, "escrow"), &escrow);
            env.storage()
                .instance()
                .set(&soroban_sdk::Symbol::new(&env, "id"), &id);
        }

        pub fn set_reenter(env: Env, reenter: bool) {
            env.storage()
                .instance()
                .set(&soroban_sdk::Symbol::new(&env, "reenter"), &reenter);
        }
    }

    #[test]
    #[should_panic(expected = "Error(Context, InvalidAction)")]
    fn test_reentrancy_lock_prevents_reentrant_release() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let token_id = env.register_contract(None, MaliciousToken);

        let client_addr = Address::generate(&env);
        let artisan_addr = Address::generate(&env);
        let arbitrator = Address::generate(&env);
        let amount = 1_000i128;
        let deadline = env.ledger().timestamp() + 86400;

        let id = client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &token_id,
            &amount,
            &0i128,
            &deadline,
            &vec![&env],
            &0u32,
            &soroban_sdk::vec![&env],
        );

        // Setup malicious token to know the escrow address and engagement id
        env.as_contract(&token_id, || {
            MaliciousToken::setup(env.clone(), contract_id.clone(), id);
        });

        // First deposit: we do not enable reentrancy yet, to allow deposit to succeed
        client.deposit(&id, &token_id);

        // Now enable reentrancy flag for the release call
        env.as_contract(&token_id, || {
            MaliciousToken::set_reenter(env.clone(), true);
        });

        // This release will trigger the token transfer, which will try to reenter release
        client.release(&id, &token_id);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #297 – Material/Labor Split tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod material_split_tests {
    use crate::{DataKey, EscrowContract, EscrowContractClient, Status};
    use soroban_sdk::testutils::{Address as AddressTestUtils, Ledger};
    use soroban_sdk::{token, vec, Address, Env};

    struct MatCtx {
        env: Env,
        contract_id: Address,
        token_address: Address,
        client: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_asset_client: token::StellarAssetClient<'static>,
    }

    impl MatCtx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let token_admin = Address::generate(&env);
            let tc = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = tc.address();
            let client = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_asset_client = token::StellarAssetClient::new(&env, &token_address);
            MatCtx {
                env,
                contract_id,
                token_address,
                client,
                token_client,
                token_asset_client,
            }
        }

        fn get_escrow(&self, id: u64) -> crate::Escrow {
            self.env.as_contract(&self.contract_id, || {
                self.env
                    .storage()
                    .persistent()
                    .get(&DataKey::Escrow(id))
                    .expect("escrow exists")
            })
        }
    }

    /// MS-1: release_materials transfers only the material_amount to artisan
    /// and marks materials_released = true.
    #[test]
    fn test_release_materials_transfers_correct_amount() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;

        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &(ctx.env.ledger().timestamp() + 86400),
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &(mat + lab));
        ctx.client.deposit(&id, &ctx.token_address);

        let artisan_balance_before = ctx.token_client.balance(&artisan);
        ctx.client
            .release_materials(&id, &ctx.token_address, &client);
        let artisan_balance_after = ctx.token_client.balance(&artisan);

        // Artisan receives exactly the material amount
        assert_eq!(artisan_balance_after, artisan_balance_before + mat);

        // Contract still holds the labor amount
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), lab);

        // State is updated
        let escrow = ctx.get_escrow(id);
        assert!(escrow.materials_released);
        assert_eq!(escrow.material_amount, mat);
        assert_eq!(escrow.labor_amount, lab);
        assert_eq!(escrow.status, Status::Funded);
    }

    /// MS-2: release_materials blocks double-spending (second call panics).
    #[test]
    #[should_panic(expected = "Materials have already been released for this engagement")]
    fn test_release_materials_double_spending_blocked() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;

        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &(ctx.env.ledger().timestamp() + 86400),
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &(mat + lab));
        ctx.client.deposit(&id, &ctx.token_address);

        // First call succeeds
        ctx.client
            .release_materials(&id, &ctx.token_address, &client);
        // Second call must panic
        ctx.client
            .release_materials(&id, &ctx.token_address, &client);
    }

    /// MS-3: release_materials can also be called by the artisan.
    #[test]
    fn test_release_materials_by_artisan() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;

        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &(ctx.env.ledger().timestamp() + 86400),
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &(mat + lab));
        ctx.client.deposit(&id, &ctx.token_address);

        let artisan_balance_before = ctx.token_client.balance(&artisan);
        // Artisan calls release_materials themselves
        ctx.client
            .release_materials(&id, &ctx.token_address, &artisan);
        assert_eq!(
            ctx.token_client.balance(&artisan),
            artisan_balance_before + mat
        );
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), lab);
    }

    /// MS-4: Unauthorized third party cannot call release_materials.
    #[test]
    #[should_panic(expected = "Only client or artisan can release materials")]
    fn test_release_materials_unauthorized_fails() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let unauthorized = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;

        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &(ctx.env.ledger().timestamp() + 86400),
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &(mat + lab));
        ctx.client.deposit(&id, &ctx.token_address);

        // Third party tries to release materials
        ctx.client
            .release_materials(&id, &ctx.token_address, &unauthorized);
    }

    /// MS-5: release after materials_released transfers only labor_amount.
    #[test]
    fn test_release_after_materials_released_transfers_labor_only() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;

        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &(ctx.env.ledger().timestamp() + 86400),
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &(mat + lab));
        ctx.client.deposit(&id, &ctx.token_address);

        // Release materials first
        ctx.client
            .release_materials(&id, &ctx.token_address, &client);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), lab);

        // Now release labor
        let artisan_before = ctx.token_client.balance(&artisan);
        ctx.client.release(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), artisan_before + lab);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);

        let escrow = ctx.get_escrow(id);
        assert_eq!(escrow.status, Status::Released);
    }

    /// MS-6: reclaim after materials_released refunds only labor_amount to client.
    #[test]
    fn test_reclaim_after_materials_released_refunds_labor_only() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;

        let deadline = ctx.env.ledger().timestamp() + 10;
        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &(mat + lab));
        ctx.client.deposit(&id, &ctx.token_address);

        // Release materials – material funds go to artisan
        ctx.client
            .release_materials(&id, &ctx.token_address, &client);
        assert_eq!(ctx.token_client.balance(&artisan), mat);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), lab);

        // Fast-forward past deadline + grace period
        ctx.env.ledger().set_timestamp(deadline + 86400 + 1);

        // Client reclaims – only gets labor back (material is already with artisan)
        let client_before = ctx.token_client.balance(&client);
        ctx.client.reclaim(&id, &ctx.token_address);
        let client_after = ctx.token_client.balance(&client);
        assert_eq!(client_after, client_before + lab);

        // Artisan keeps the material amount they received
        assert_eq!(ctx.token_client.balance(&artisan), mat);

        // Contract is empty
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);

        let escrow = ctx.get_escrow(id);
        assert_eq!(escrow.status, Status::Refunded);
    }

    /// MS-7: reclaim without materials_released refunds the full amount.
    #[test]
    fn test_reclaim_without_materials_released_refunds_full() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;
        let total = mat + lab;

        let deadline = ctx.env.ledger().timestamp() + 10;
        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &total);
        ctx.client.deposit(&id, &ctx.token_address);

        // Fast-forward past deadline + grace period (no release_materials called)
        ctx.env.ledger().set_timestamp(deadline + 86400 + 1);

        let client_before = ctx.token_client.balance(&client);
        ctx.client.reclaim(&id, &ctx.token_address);
        assert_eq!(
            ctx.token_client.balance(&client),
            client_before + total,
            "Client should get full refund when materials not released"
        );
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);

        let escrow = ctx.get_escrow(id);
        assert_eq!(escrow.status, Status::Refunded);
    }

    /// MS-8: release without calling release_materials transfers everything.
    #[test]
    fn test_release_without_materials_released_transfers_full() {
        let ctx = MatCtx::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let mat = 1_000i128;
        let lab = 2_000i128;
        let total = mat + lab;

        let id = ctx.client.initialize(
            &client,
            &artisan,
            &Address::generate(&ctx.env),
            &ctx.token_address,
            &mat,
            &lab,
            &(ctx.env.ledger().timestamp() + 86400),
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client, &total);
        ctx.client.deposit(&id, &ctx.token_address);

        let artisan_before = ctx.token_client.balance(&artisan);
        ctx.client.release(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), artisan_before + total);

        let escrow = ctx.get_escrow(id);
        assert_eq!(escrow.status, Status::Released);
        assert!(!escrow.materials_released);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #XXX – Protocol fee / treasury tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod fee_tests {
    use crate::{DataKey, EscrowContract, EscrowContractClient, Status};
    use soroban_sdk::testutils::Address as AddressTestUtils;
    use soroban_sdk::{token, vec, Address, Env};

    struct FeeCtx {
        env: Env,
        contract_id: Address,
        token_address: Address,
        admin: Address,
        client: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_asset_client: token::StellarAssetClient<'static>,
    }

    impl FeeCtx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let admin = Address::generate(&env);
            let token_admin = Address::generate(&env);
            let tc = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = tc.address();
            let client = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_asset_client = token::StellarAssetClient::new(&env, &token_address);

            client.init_admin(&admin);

            FeeCtx {
                env,
                contract_id,
                token_address,
                admin,
                client,
                token_client,
                token_asset_client,
            }
        }

        fn set_treasury(&self, treasury: &Address, bps: u32) {
            self.client.init_treasury(&self.admin, treasury, &bps);
        }

        fn fund_engagement(&self, material: i128, labor: i128) -> (u64, Address, Address) {
            let client_addr = Address::generate(&self.env);
            let artisan_addr = Address::generate(&self.env);
            let arbitrator = Address::generate(&self.env);
            let deadline = self.env.ledger().timestamp() + 86400;
            let id = self.client.initialize(
                &client_addr,
                &artisan_addr,
                &arbitrator,
                &self.token_address,
                &material,
                &labor,
                &deadline,
                &vec![&self.env],
                &0u32,
                &soroban_sdk::vec![&self.env],
            );
            self.token_asset_client
                .mint(&client_addr, &(material + labor));
            self.client.deposit(&id, &self.token_address);
            (id, client_addr, artisan_addr)
        }
    }

    /// F-1: Admin can set treasury + fee rate, and it's readable back.
    #[test]
    fn test_init_treasury_sets_config() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 250);

        let cfg = ctx.client.get_treasury().unwrap();
        assert_eq!(cfg.treasury_address, treasury);
        assert_eq!(cfg.fee_basis_points, 250);
    }

    /// F-2: Non-admin cannot configure treasury.
    #[test]
    #[should_panic(expected = "Only admin can configure treasury")]
    fn test_init_treasury_unauthorized_fails() {
        let ctx = FeeCtx::new();
        let not_admin = Address::generate(&ctx.env);
        let treasury = Address::generate(&ctx.env);
        ctx.client.init_treasury(&not_admin, &treasury, &250);
    }

    /// F-3: Fee rate above the 10% cap is rejected.
    #[test]
    #[should_panic(expected = "fee_basis_points exceeds maximum allowed")]
    fn test_init_treasury_exceeds_max_bps_fails() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 1001);
    }

    /// F-4: release() splits exactly — 2.5% on a clean amount, no dust in contract.
    #[test]
    fn test_release_deducts_fee_exact_math() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 250); // 2.5%

        let (id, _client, artisan) = ctx.fund_engagement(10_000, 0);
        ctx.client.release(&id, &ctx.token_address);

        assert_eq!(ctx.token_client.balance(&treasury), 250);
        assert_eq!(ctx.token_client.balance(&artisan), 9_750);
        assert_eq!(
            ctx.token_client.balance(&ctx.contract_id),
            0,
            "no dust left behind"
        );

        let escrow: crate::Escrow = ctx.env.as_contract(&ctx.contract_id, || {
            ctx.env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(id))
                .unwrap()
        });
        assert_eq!(escrow.status, Status::Released);
    }

    /// F-5: Fee rounds down (floor); artisan absorbs the fractional remainder,
    /// so fee + artisan payout always equals the original amount exactly.
    #[test]
    fn test_release_fee_rounding_no_dust() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 250); // 2.5%

        let amount: i128 = 1_001; // 1001 * 250 / 10000 = 25.025 -> floors to 25
        let (id, _client, artisan) = ctx.fund_engagement(amount, 0);
        ctx.client.release(&id, &ctx.token_address);

        let fee = ctx.token_client.balance(&treasury);
        let artisan_payout = ctx.token_client.balance(&artisan);

        assert_eq!(fee, 25);
        assert_eq!(artisan_payout, 976);
        assert_eq!(
            fee + artisan_payout,
            amount,
            "fee + payout must equal original amount"
        );
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }

    /// F-6: With no treasury configured, behavior is unchanged (100% to artisan).
    #[test]
    fn test_release_without_treasury_configured_full_amount_to_artisan() {
        let ctx = FeeCtx::new();
        let (id, _client, artisan) = ctx.fund_engagement(5_000, 0);
        ctx.client.release(&id, &ctx.token_address);

        assert_eq!(ctx.token_client.balance(&artisan), 5_000);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }

    /// F-7: resolve_dispute() split — fee is deducted only from the artisan's
    /// share; the client's share is untouched.
    #[test]
    fn test_resolve_dispute_fee_applied_to_artisan_share_only() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 250); // 2.5%

        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 10_000;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);
        ctx.client.dispute(&id, &client_addr);

        // 60/40 split: client gets 6000 untouched, artisan share (4000) gets fee'd.
        ctx.client
            .resolve_dispute(&id, &6_000, &4_000, &ctx.token_address);

        assert_eq!(
            ctx.token_client.balance(&client_addr),
            6_000,
            "client refund is never fee'd"
        );
        assert_eq!(ctx.token_client.balance(&treasury), 100); // 2.5% of 4000
        assert_eq!(ctx.token_client.balance(&artisan_addr), 3_900);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }

    /// F-8: A full refund to the client (artisan_amount == 0) is never fee'd.
    #[test]
    fn test_resolve_dispute_full_refund_no_fee() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 250);

        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 5_000;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);
        ctx.client.dispute(&id, &artisan_addr);

        ctx.client
            .resolve_dispute(&id, &amount, &0, &ctx.token_address);

        assert_eq!(ctx.token_client.balance(&client_addr), amount);
        assert_eq!(
            ctx.token_client.balance(&treasury),
            0,
            "no fee on a full client refund"
        );
    }

    /// F-9: A full release via dispute (client_amount == 0) still gets fee'd.
    #[test]
    fn test_resolve_dispute_full_release_fee_applied() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 250);

        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let amount: i128 = 8_000;
        let deadline = ctx.env.ledger().timestamp() + 86400;

        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &amount,
            &0i128,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &amount);
        ctx.client.deposit(&id, &ctx.token_address);
        ctx.client.dispute(&id, &client_addr);

        ctx.client
            .resolve_dispute(&id, &0, &amount, &ctx.token_address);

        assert_eq!(ctx.token_client.balance(&treasury), 200); // 2.5% of 8000
        assert_eq!(ctx.token_client.balance(&artisan_addr), 7_800);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }

    /// F-10: Invariant sweep — for a range of odd amounts, fee + artisan payout
    /// always reconstructs the original amount exactly, and no dust is ever
    /// left in the contract.
    #[test]
    fn test_fee_math_no_dust_across_many_amounts() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 333); // an intentionally "ugly" 3.33%

        for amount in [1i128, 7, 99, 1_001, 123_457, 999_999, 10_000_000] {
            let (id, _client, artisan) = ctx.fund_engagement(amount, 0);
            let treasury_before = ctx.token_client.balance(&treasury);
            let artisan_before = ctx.token_client.balance(&artisan);

            ctx.client.release(&id, &ctx.token_address);

            let fee = ctx.token_client.balance(&treasury) - treasury_before;
            let payout = ctx.token_client.balance(&artisan) - artisan_before;

            assert_eq!(
                fee + payout,
                amount,
                "amount={amount}: fee+payout must equal amount"
            );
            assert_eq!(
                ctx.token_client.balance(&ctx.contract_id),
                0,
                "amount={amount}: no dust"
            );
        }
    }

    /// F-11: Materials-released path still respects the fee on the labor remainder.
    #[test]
    fn test_release_fee_applies_to_remaining_labor_after_materials_released() {
        let ctx = FeeCtx::new();
        let treasury = Address::generate(&ctx.env);
        ctx.set_treasury(&treasury, 250);

        let (id, client_addr, artisan) = ctx.fund_engagement(2_000, 8_000);
        ctx.client
            .release_materials(&id, &ctx.token_address, &client_addr);

        // Materials (2000) went to artisan fee-free — only release() fees the payout.
        assert_eq!(ctx.token_client.balance(&artisan), 2_000);

        ctx.client.release(&id, &ctx.token_address);

        // Fee applies only to the labor_amount (8000) remaining in the contract.
        assert_eq!(ctx.token_client.balance(&treasury), 200); // 2.5% of 8000
        assert_eq!(ctx.token_client.balance(&artisan), 2_000 + 7_800);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Milestone-based partial release tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod milestone_tests {
    use crate::{EscrowContract, EscrowContractClient, Status};
    use soroban_sdk::testutils::{Address as AddressTestUtils, Ledger};
    use soroban_sdk::{token, vec, Address, Env};

    struct MilestoneCtx {
        env: Env,
        contract_id: Address,
        token_address: Address,
        client: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_asset_client: token::StellarAssetClient<'static>,
    }

    impl MilestoneCtx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let token_admin = Address::generate(&env);
            let tc = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = tc.address();
            let client = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_asset_client = token::StellarAssetClient::new(&env, &token_address);
            Self {
                env,
                contract_id,
                token_address,
                client,
                token_client,
                token_asset_client,
            }
        }

        fn fund_with_milestones(
            &self,
            material: i128,
            labor: i128,
            milestones: soroban_sdk::Vec<u32>,
        ) -> (u64, Address, Address) {
            let client_addr = Address::generate(&self.env);
            let artisan = Address::generate(&self.env);
            let arbitrator = Address::generate(&self.env);
            let deadline = self.env.ledger().timestamp() + 86_400;
            let id = self.client.initialize(
                &client_addr,
                &artisan,
                &arbitrator,
                &self.token_address,
                &material,
                &labor,
                &deadline,
                &vec![&self.env],
                &0u32,
                &milestones,
            );
            let total = material + labor;
            self.token_asset_client.mint(&client_addr, &total);
            self.client.deposit(&id, &self.token_address);
            (id, client_addr, artisan)
        }
    }

    #[test]
    fn test_milestone_multi_stage_release_flow() {
        let ctx = MilestoneCtx::new();
        let milestones = vec![&ctx.env, 25u32, 25u32, 50u32];
        let total = 10_000i128;
        let (id, _client, artisan) = ctx.fund_with_milestones(total, 0, milestones);

        assert_eq!(ctx.client.get_next_milestone(&id), 0);
        assert_eq!(
            ctx.client.get_milestones(&id),
            vec![&ctx.env, 25u32, 25u32, 50u32]
        );

        ctx.client.release_milestone(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), 2_500);
        assert_eq!(ctx.client.get_next_milestone(&id), 1);
        assert_eq!(ctx.client.get_engagement(&id).status, Status::Funded);

        ctx.client.release_milestone(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), 5_000);
        assert_eq!(ctx.client.get_next_milestone(&id), 2);

        ctx.client.release_milestone(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), 10_000);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
        assert_eq!(ctx.client.get_next_milestone(&id), 3);
        assert_eq!(ctx.client.get_engagement(&id).status, Status::Released);
    }

    #[test]
    fn test_milestone_last_payout_absorbs_rounding_dust() {
        let ctx = MilestoneCtx::new();
        // 33+33+34 = 100; total 1000 → 330 + 330 + 340
        let milestones = vec![&ctx.env, 33u32, 33u32, 34u32];
        let (id, _client, artisan) = ctx.fund_with_milestones(1_000, 0, milestones);

        ctx.client.release_milestone(&id, &ctx.token_address);
        ctx.client.release_milestone(&id, &ctx.token_address);
        ctx.client.release_milestone(&id, &ctx.token_address);

        assert_eq!(ctx.token_client.balance(&artisan), 1_000);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    }

    #[test]
    #[should_panic(expected = "All milestones have already been released")]
    fn test_milestone_cannot_release_past_final() {
        let ctx = MilestoneCtx::new();
        let milestones = vec![&ctx.env, 50u32, 50u32];
        let (id, _client, _artisan) = ctx.fund_with_milestones(1_000, 0, milestones);

        ctx.client.release_milestone(&id, &ctx.token_address);
        ctx.client.release_milestone(&id, &ctx.token_address);
        // Third call — nothing left
        ctx.client.release_milestone(&id, &ctx.token_address);
    }

    #[test]
    #[should_panic(expected = "Use release_milestone for milestone-based escrows")]
    fn test_milestone_blocks_legacy_release() {
        let ctx = MilestoneCtx::new();
        let milestones = vec![&ctx.env, 40u32, 60u32];
        let (id, _client, _artisan) = ctx.fund_with_milestones(1_000, 0, milestones);
        ctx.client.release(&id, &ctx.token_address);
    }

    #[test]
    #[should_panic(expected = "Use release_milestone for milestone-based escrows")]
    fn test_milestone_blocks_release_materials() {
        let ctx = MilestoneCtx::new();
        let milestones = vec![&ctx.env, 40u32, 60u32];
        let (id, client_addr, _artisan) = ctx.fund_with_milestones(400, 600, milestones);
        ctx.client
            .release_materials(&id, &ctx.token_address, &client_addr);
    }

    #[test]
    #[should_panic(expected = "Escrow has no milestones configured")]
    fn test_release_milestone_requires_milestones() {
        let ctx = MilestoneCtx::new();
        let (id, _client, _artisan) = ctx.fund_with_milestones(1_000, 0, vec![&ctx.env]);
        ctx.client.release_milestone(&id, &ctx.token_address);
    }

    #[test]
    #[should_panic(expected = "Milestone percentages must sum to exactly 100")]
    fn test_milestones_must_sum_to_100() {
        let ctx = MilestoneCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 86_400;
        ctx.client.initialize(
            &client_addr,
            &artisan,
            &arbitrator,
            &ctx.token_address,
            &1_000i128,
            &0i128,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &vec![&ctx.env, 25u32, 25u32, 25u32], // 75 != 100
        );
    }

    #[test]
    #[should_panic(expected = "Milestone percentages must be greater than zero")]
    fn test_milestone_zero_percentage_rejected() {
        let ctx = MilestoneCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 86_400;
        ctx.client.initialize(
            &client_addr,
            &artisan,
            &arbitrator,
            &ctx.token_address,
            &1_000i128,
            &0i128,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &vec![&ctx.env, 0u32, 100u32],
        );
    }

    #[test]
    fn test_milestone_reclaim_returns_only_remaining() {
        let ctx = MilestoneCtx::new();
        let milestones = vec![&ctx.env, 25u32, 75u32];
        let total = 4_000i128;
        let (id, client_addr, artisan) = ctx.fund_with_milestones(total, 0, milestones);

        ctx.client.release_milestone(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), 1_000);

        // Jump past deadline + grace period (24h)
        ctx.env.ledger().with_mut(|li| {
            li.timestamp += 86_400 + 86_400 + 1;
        });

        assert!(ctx.client.reclaim(&id, &ctx.token_address));
        assert_eq!(ctx.token_client.balance(&client_addr), 3_000);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
        assert_eq!(ctx.client.get_engagement(&id).status, Status::Refunded);
    }

    #[test]
    fn test_sequential_milestones_cannot_skip_ahead() {
        // release_milestone always pays the current index; there is no way to
        // request milestone 2 before milestone 1. Verify ordering via index.
        let ctx = MilestoneCtx::new();
        let milestones = vec![&ctx.env, 10u32, 20u32, 70u32];
        let (id, _client, artisan) = ctx.fund_with_milestones(1_000, 0, milestones);

        assert_eq!(ctx.client.get_next_milestone(&id), 0);
        ctx.client.release_milestone(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), 100); // 10%
        assert_eq!(ctx.client.get_next_milestone(&id), 1);

        ctx.client.release_milestone(&id, &ctx.token_address);
        assert_eq!(ctx.token_client.balance(&artisan), 300); // +20%
        assert_eq!(ctx.client.get_next_milestone(&id), 2);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #373 – DAO dispute resolution (jury system) escrow-side tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod dao_dispute_tests {
    use crate::{EscrowContract, EscrowContractClient, Status};
    use soroban_sdk::testutils::Address as AddressTestUtils;
    use soroban_sdk::{contract, contractimpl, token, vec, Address, Env};

    /// Minimal probe contract used to verify that `resolve_dispute_dao`
    /// accepts a real cross-contract call from the registered resolver with
    /// mock auths disabled (the production auth path).
    #[contract]
    struct DaoProbe;

    #[contractimpl]
    impl DaoProbe {
        pub fn resolve(
            env: Env,
            escrow_contract: Address,
            engagement_id: u64,
            client_amount: i128,
            artisan_amount: i128,
            token: Address,
        ) -> i128 {
            crate::EscrowContractClient::new(&env, &escrow_contract).resolve_dispute_dao(
                &engagement_id,
                &client_amount,
                &artisan_amount,
                &token,
            )
        }
    }

    struct DaoCtx {
        env: Env,
        contract_id: Address,
        token_address: Address,
        admin: Address,
        resolver: Address,
        treasury: Address,
        client: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_asset_client: token::StellarAssetClient<'static>,
    }

    impl DaoCtx {
        /// Setup with treasury (2.5%), jury reward (50% of fee) and a default
        /// registered resolver.
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let admin = Address::generate(&env);
            let resolver = Address::generate(&env);
            let treasury = Address::generate(&env);
            let token_admin = Address::generate(&env);
            let tc = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = tc.address();
            let client = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_asset_client = token::StellarAssetClient::new(&env, &token_address);

            client.init_admin(&admin);
            client.set_dispute_resolver(&admin, &resolver);
            client.set_jury_reward_bps(&admin, &5_000);
            client.init_treasury(&admin, &treasury, &250);

            DaoCtx {
                env,
                contract_id,
                token_address,
                admin,
                resolver,
                treasury,
                client,
                token_client,
                token_asset_client,
            }
        }

        /// Create a funded and disputed escrow. Returns (id, client, artisan).
        fn fund_and_dispute(&self, amount: i128) -> (u64, Address, Address) {
            let client_addr = Address::generate(&self.env);
            let artisan_addr = Address::generate(&self.env);
            let arbitrator = Address::generate(&self.env);
            let deadline = self.env.ledger().timestamp() + 86_400;
            let id = self.client.initialize(
                &client_addr,
                &artisan_addr,
                &arbitrator,
                &self.token_address,
                &amount,
                &0i128,
                &deadline,
                &vec![&self.env],
                &0u32,
                &vec![&self.env],
            );
            self.token_asset_client.mint(&client_addr, &amount);
            self.client.deposit(&id, &self.token_address);
            self.client.dispute(&id, &client_addr);
            (id, client_addr, artisan_addr)
        }
    }

    /// DAO-1: Admin can register the DAO dispute resolver; it is readable.
    #[test]
    fn test_set_dispute_resolver_and_read() {
        let ctx = DaoCtx::new();
        let resolver = Address::generate(&ctx.env);
        ctx.client.set_dispute_resolver(&ctx.admin, &resolver);
        assert_eq!(ctx.client.get_dispute_resolver().unwrap(), resolver);
    }

    /// DAO-2: Non-admin cannot register the dispute resolver.
    #[test]
    #[should_panic(expected = "Only admin can configure the dispute resolver")]
    fn test_set_dispute_resolver_unauthorized_fails() {
        let ctx = DaoCtx::new();
        let not_admin = Address::generate(&ctx.env);
        let resolver = Address::generate(&ctx.env);
        ctx.client.set_dispute_resolver(&not_admin, &resolver);
    }

    /// DAO-3: Jury reward share of the fee is configurable by the admin only.
    #[test]
    fn test_set_jury_reward_bps_and_read() {
        let ctx = DaoCtx::new();
        ctx.client.set_jury_reward_bps(&ctx.admin, &2_500);
        assert_eq!(ctx.client.get_jury_reward_bps(), 2_500);
    }

    /// DAO-4: Non-admin cannot configure the jury reward share.
    #[test]
    #[should_panic(expected = "Only admin can configure jury rewards")]
    fn test_set_jury_reward_bps_unauthorized_fails() {
        let ctx = DaoCtx::new();
        let not_admin = Address::generate(&ctx.env);
        ctx.client.set_jury_reward_bps(&not_admin, &2_500);
    }

    /// DAO-5: Jury reward share cannot exceed 100% of the fee.
    #[test]
    #[should_panic(expected = "jury_reward_bps exceeds maximum allowed")]
    fn test_set_jury_reward_bps_exceeds_max_fails() {
        let ctx = DaoCtx::new();
        ctx.client.set_jury_reward_bps(&ctx.admin, &10_001);
    }

    /// DAO-6: get_remaining_balance reports the locked amount.
    #[test]
    fn test_get_remaining_balance() {
        let ctx = DaoCtx::new();
        let (id, _, _) = ctx.fund_and_dispute(10_000);
        assert_eq!(ctx.client.get_remaining_balance(&id), 10_000);
    }

    /// DAO-7: resolve_dispute_dao rejects callers that are not the resolver.
    #[test]
    #[should_panic]
    fn test_resolve_dispute_dao_unauthorized_fails() {
        let ctx = DaoCtx::new();
        let (id, _, _) = ctx.fund_and_dispute(10_000);
        // Clear mock auths: a plain user is not the registered resolver.
        ctx.env.set_auths(&[]);
        ctx.client
            .resolve_dispute_dao(&id, &0i128, &10_000i128, &ctx.token_address);
    }

    /// DAO-8: Only Disputed escrows can be resolved by the DAO.
    #[test]
    #[should_panic(expected = "Escrow must be in Disputed status to resolve")]
    fn test_resolve_dispute_dao_requires_disputed_state() {
        let ctx = DaoCtx::new();
        let client_addr = Address::generate(&ctx.env);
        let artisan_addr = Address::generate(&ctx.env);
        let arbitrator = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 86_400;
        let id = ctx.client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &ctx.token_address,
            &10_000i128,
            &0i128,
            &deadline,
            &vec![&ctx.env],
            &0u32,
            &vec![&ctx.env],
        );
        ctx.token_asset_client.mint(&client_addr, &10_000i128);
        ctx.client.deposit(&id, &ctx.token_address);
        // Escrow is Funded, not Disputed.
        ctx.client
            .resolve_dispute_dao(&id, &0i128, &10_000i128, &ctx.token_address);
    }

    /// DAO-9: Distribution amounts must sum to the remaining balance.
    #[test]
    #[should_panic(expected = "Distribution amounts must equal the remaining escrowed amount")]
    fn test_resolve_dispute_dao_invalid_amounts_fail() {
        let ctx = DaoCtx::new();
        let (id, _, _) = ctx.fund_and_dispute(10_000);
        ctx.client
            .resolve_dispute_dao(&id, &3_000i128, &3_000i128, &ctx.token_address);
    }

    /// DAO-10: Negative distribution amounts are rejected.
    #[test]
    #[should_panic(expected = "Distribution amounts must be non-negative")]
    fn test_resolve_dispute_dao_negative_amounts_fail() {
        let ctx = DaoCtx::new();
        let (id, _, _) = ctx.fund_and_dispute(10_000);
        ctx.client
            .resolve_dispute_dao(&id, &(-1_000i128), &11_000i128, &ctx.token_address);
    }

    /// DAO-11: The token must match the escrow's token.
    #[test]
    #[should_panic(expected = "Token does not match the initialized token for this engagement")]
    fn test_resolve_dispute_dao_token_mismatch_fails() {
        let ctx = DaoCtx::new();
        let (id, _, _) = ctx.fund_and_dispute(10_000);
        let other_token = Address::generate(&ctx.env);
        ctx.client
            .resolve_dispute_dao(&id, &0i128, &10_000i128, &other_token);
    }

    /// DAO-12: Full release — fee is split between treasury and jury pool;
    /// returns the jury reward amount; escrow transitions to Released.
    #[test]
    fn test_resolve_dispute_dao_full_release_fee_split() {
        let ctx = DaoCtx::new();
        let (id, _, artisan) = ctx.fund_and_dispute(10_000);

        let jury_reward =
            ctx.client
                .resolve_dispute_dao(&id, &0i128, &10_000i128, &ctx.token_address);

        // fee = 2.5% of 10,000 = 250; jury gets 50% (125), treasury 125.
        assert_eq!(jury_reward, 125);
        assert_eq!(ctx.token_client.balance(&ctx.resolver), 125);
        assert_eq!(ctx.token_client.balance(&ctx.treasury), 125);
        assert_eq!(ctx.token_client.balance(&artisan), 9_750);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
        assert_eq!(ctx.client.get_engagement(&id).status, Status::Released);
    }

    /// DAO-13: Full refund — client refunds are fee-free; no jury reward.
    #[test]
    fn test_resolve_dispute_dao_full_refund_no_fee() {
        let ctx = DaoCtx::new();
        let (id, client_addr, _) = ctx.fund_and_dispute(10_000);

        let jury_reward =
            ctx.client
                .resolve_dispute_dao(&id, &10_000i128, &0i128, &ctx.token_address);

        assert_eq!(jury_reward, 0);
        assert_eq!(ctx.token_client.balance(&client_addr), 10_000);
        assert_eq!(ctx.token_client.balance(&ctx.treasury), 0);
        assert_eq!(ctx.token_client.balance(&ctx.resolver), 0);
        assert_eq!(ctx.client.get_engagement(&id).status, Status::Refunded);
    }

    /// DAO-14: Split distribution transitions the escrow to Resolved.
    #[test]
    fn test_resolve_dispute_dao_split_status_resolved() {
        let ctx = DaoCtx::new();
        let (id, client_addr, artisan) = ctx.fund_and_dispute(10_000);

        let jury_reward =
            ctx.client
                .resolve_dispute_dao(&id, &4_000i128, &6_000i128, &ctx.token_address);

        // fee = 2.5% of 6,000 = 150, deducted from the artisan payout;
        // the fee is then split: jury 75, treasury 75.
        assert_eq!(jury_reward, 75);
        assert_eq!(ctx.token_client.balance(&client_addr), 4_000);
        assert_eq!(ctx.token_client.balance(&artisan), 5_850);
        assert_eq!(ctx.token_client.balance(&ctx.treasury), 75);
        assert_eq!(ctx.token_client.balance(&ctx.resolver), 75);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
        assert_eq!(ctx.client.get_engagement(&id).status, Status::Resolved);
    }

    /// DAO-15: With no treasury configured there is no fee and no jury pool.
    #[test]
    fn test_resolve_dispute_dao_without_treasury() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register_contract(None, EscrowContract);
        let admin = Address::generate(&env);
        let resolver = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let tc = env.register_stellar_asset_contract_v2(token_admin);
        let token_address = tc.address();
        let client = EscrowContractClient::new(&env, &contract_id);
        let token_asset_client = token::StellarAssetClient::new(&env, &token_address);

        client.init_admin(&admin);
        client.set_dispute_resolver(&admin, &resolver);

        let client_addr = Address::generate(&env);
        let artisan_addr = Address::generate(&env);
        let arbitrator = Address::generate(&env);
        let deadline = env.ledger().timestamp() + 86_400;
        let id = client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &token_address,
            &10_000i128,
            &0i128,
            &deadline,
            &vec![&env],
            &0u32,
            &vec![&env],
        );
        token_asset_client.mint(&client_addr, &10_000i128);
        client.deposit(&id, &token_address);
        client.dispute(&id, &client_addr);

        let jury_reward = client.resolve_dispute_dao(&id, &0i128, &10_000i128, &token_address);

        assert_eq!(jury_reward, 0);
        assert_eq!(
            token::Client::new(&env, &token_address).balance(&artisan_addr),
            10_000
        );
        assert_eq!(
            token::Client::new(&env, &token_address).balance(&resolver),
            0
        );
        assert_eq!(client.get_engagement(&id).status, Status::Released);
    }

    /// DAO-16: With a zero jury share, the whole fee goes to the treasury.
    #[test]
    fn test_resolve_dispute_dao_zero_jury_bps() {
        let ctx = DaoCtx::new();
        ctx.client.set_jury_reward_bps(&ctx.admin, &0);
        let (id, _, _) = ctx.fund_and_dispute(10_000);

        let jury_reward =
            ctx.client
                .resolve_dispute_dao(&id, &0i128, &10_000i128, &ctx.token_address);

        assert_eq!(jury_reward, 0);
        assert_eq!(ctx.token_client.balance(&ctx.treasury), 250);
        assert_eq!(ctx.token_client.balance(&ctx.resolver), 0);
    }

    /// DAO-17: A registered resolver contract can resolve the dispute with
    /// real (non-mocked) cross-contract authorization.
    #[test]
    fn test_resolve_dispute_dao_accepts_registered_resolver_contract() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register_contract(None, EscrowContract);
        let probe_id = env.register_contract(None, DaoProbe);
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let tc = env.register_stellar_asset_contract_v2(token_admin);
        let token_address = tc.address();
        let client = EscrowContractClient::new(&env, &contract_id);
        let token_client = token::Client::new(&env, &token_address);
        let token_asset_client = token::StellarAssetClient::new(&env, &token_address);

        client.init_admin(&admin);
        // The probe contract is the registered DAO resolver.
        client.set_dispute_resolver(&admin, &probe_id);
        client.set_jury_reward_bps(&admin, &5_000);
        let treasury = Address::generate(&env);
        client.init_treasury(&admin, &treasury, &250);

        let client_addr = Address::generate(&env);
        let artisan_addr = Address::generate(&env);
        let arbitrator = Address::generate(&env);
        let deadline = env.ledger().timestamp() + 86_400;
        let id = client.initialize(
            &client_addr,
            &artisan_addr,
            &arbitrator,
            &token_address,
            &10_000i128,
            &0i128,
            &deadline,
            &vec![&env],
            &0u32,
            &vec![&env],
        );
        token_asset_client.mint(&client_addr, &10_000i128);
        client.deposit(&id, &token_address);
        client.dispute(&id, &client_addr);

        // Disable mock auths: the cross-contract call must carry the probe's
        // own authorization (the production auth path).
        env.set_auths(&[]);

        let jury_reward = DaoProbeClient::new(&env, &probe_id).resolve(
            &contract_id,
            &id,
            &0i128,
            &10_000i128,
            &token_address,
        );

        assert_eq!(jury_reward, 125);
        assert_eq!(token_client.balance(&probe_id), 125);
        assert_eq!(token_client.balance(&treasury), 125);
        assert_eq!(token_client.balance(&artisan_addr), 9_750);
        assert_eq!(token_client.balance(&contract_id), 0);
        assert_eq!(client.get_engagement(&id).status, Status::Released);
    }
}

#[cfg(test)]
mod submission_pending_tests {
    use crate::{DataKey, Escrow, EscrowContract, EscrowContractClient, Status, GRACE_PERIOD, SUBMISSION_GRACE_PERIOD};
    use soroban_sdk::testutils::{Address as AddressTestUtils, Ledger};
    use soroban_sdk::{token, Address, Env};

    struct TestContext {
        env: Env,
        contract_id: Address,
        token_address: Address,
        default_arbitrator: Address,
        client_contract: EscrowContractClient<'static>,
        token_client: token::Client<'static>,
        token_contract_client: token::StellarAssetClient<'static>,
    }

    impl TestContext {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths_allowing_non_root_auth();
            let contract_id = env.register_contract(None, EscrowContract);
            let token_admin = Address::generate(&env);
            let token_contract = env.register_stellar_asset_contract_v2(token_admin);
            let token_address = token_contract.address();
            let default_arbitrator = Address::generate(&env);

            let client_contract = EscrowContractClient::new(&env, &contract_id);
            let token_client = token::Client::new(&env, &token_address);
            let token_contract_client = token::StellarAssetClient::new(&env, &token_address);

            TestContext {
                env,
                contract_id,
                token_address,
                default_arbitrator,
                client_contract,
                token_client,
                token_contract_client,
            }
        }

        fn get_escrow(&self, engagement_id: u64) -> Escrow {
            self.env.as_contract(&self.contract_id, || {
                self.env
                    .storage()
                    .persistent()
                    .get(&DataKey::Escrow(engagement_id))
                    .expect("Escrow should exist")
            })
        }

        fn setup_funded_escrow(
            &self,
            material: i128,
            labor: i128,
            deadline_offset: u64,
        ) -> (Address, Address, u64) {
            let client = Address::generate(&self.env);
            let artisan = Address::generate(&self.env);
            let deadline = self.env.ledger().timestamp() + deadline_offset;
            let id = self.client_contract.initialize(
                &client,
                &artisan,
                &self.default_arbitrator,
                &self.token_address,
                &material,
                &labor,
                &deadline,
                &soroban_sdk::vec![&self.env],
                &0u32,
                &soroban_sdk::vec![&self.env],
            );
            self.token_contract_client.mint(&client, &(material + labor));
            self.client_contract.deposit(&id, &self.token_address);
            (client, artisan, id)
        }
    }

    #[test]
    fn test_artisan_submit_work_transitions_to_submission_pending() {
        let ctx = TestContext::new();
        let (_client, _artisan, id) = ctx.setup_funded_escrow(1_000, 4_000, 86_400);

        assert_eq!(ctx.get_escrow(id).status, Status::Funded);

        ctx.client_contract.submit_work(&id);

        let escrow = ctx.get_escrow(id);
        assert_eq!(escrow.status, Status::SubmissionPending);

        // Verify storage key exists
        ctx.env.as_contract(&ctx.contract_id, || {
            let stored_time: Option<u64> = ctx.env.storage().persistent().get(&DataKey::SubmissionPending(id));
            assert!(stored_time.is_some());
        });
    }

    #[test]
    fn test_trigger_submission_pending_transitions_state() {
        let ctx = TestContext::new();
        let (_client, _artisan, id) = ctx.setup_funded_escrow(1_000, 4_000, 86_400);

        ctx.client_contract.trigger_submission_pending(&id);

        let escrow = ctx.get_escrow(id);
        assert_eq!(escrow.status, Status::SubmissionPending);
    }

    #[test]
    #[should_panic(expected = "Escrow must be Funded or InProgress to submit work")]
    fn test_submit_work_fails_if_not_funded() {
        let ctx = TestContext::new();
        let client = Address::generate(&ctx.env);
        let artisan = Address::generate(&ctx.env);
        let deadline = ctx.env.ledger().timestamp() + 86_400;
        let id = ctx.client_contract.initialize(
            &client,
            &artisan,
            &ctx.default_arbitrator,
            &ctx.token_address,
            &1_000,
            &4_000,
            &deadline,
            &soroban_sdk::vec![&ctx.env],
            &0u32,
            &soroban_sdk::vec![&ctx.env],
        );
        // Not deposited, status is Pending
        ctx.client_contract.submit_work(&id);
    }

    #[test]
    #[should_panic(expected = "Submission Pending is active; reclaim is locked for an additional 12 hours")]
    fn test_reclaim_blocked_during_submission_pending_grace_period() {
        let ctx = TestContext::new();
        let deadline_offset = 10;
        let (_client, _artisan, id) = ctx.setup_funded_escrow(1_000, 4_000, deadline_offset);

        let deadline = ctx.get_escrow(id).deadline;

        // Artisan submits work
        ctx.client_contract.submit_work(&id);
        assert_eq!(ctx.get_escrow(id).status, Status::SubmissionPending);

        // Fast forward past normal deadline + GRACE_PERIOD (24h)
        // Normal reclaim would succeed at deadline + 86400 + 1
        ctx.env.ledger().set_timestamp(deadline + GRACE_PERIOD + 1);

        // Reclaim should fail because extra 12h SUBMISSION_GRACE_PERIOD is active
        ctx.client_contract.reclaim(&id, &ctx.token_address);
    }

    #[test]
    fn test_reclaim_succeeds_after_submission_pending_grace_period_elapses() {
        let ctx = TestContext::new();
        let deadline_offset = 10;
        let (client, _artisan, id) = ctx.setup_funded_escrow(1_000, 4_000, deadline_offset);

        let deadline = ctx.get_escrow(id).deadline;

        // Artisan submits work
        ctx.client_contract.submit_work(&id);
        assert_eq!(ctx.get_escrow(id).status, Status::SubmissionPending);

        let client_bal_before = ctx.token_client.balance(&client);

        // Fast forward past deadline + GRACE_PERIOD (24h) + SUBMISSION_GRACE_PERIOD (12h) + 1s
        ctx.env.ledger().set_timestamp(deadline + GRACE_PERIOD + SUBMISSION_GRACE_PERIOD + 1);

        // Reclaim should now succeed
        ctx.client_contract.reclaim(&id, &ctx.token_address);

        let client_bal_after = ctx.token_client.balance(&client);
        assert_eq!(client_bal_after, client_bal_before + 5_000);
        assert_eq!(ctx.get_escrow(id).status, Status::Refunded);

        // Storage key should be cleaned up
        ctx.env.as_contract(&ctx.contract_id, || {
            let stored_time: Option<u64> = ctx.env.storage().persistent().get(&DataKey::SubmissionPending(id));
            assert!(stored_time.is_none());
        });
    }

    #[test]
    fn test_client_release_succeeds_from_submission_pending() {
        let ctx = TestContext::new();
        let (_client, artisan, id) = ctx.setup_funded_escrow(1_000, 4_000, 86_400);

        ctx.client_contract.submit_work(&id);
        assert_eq!(ctx.get_escrow(id).status, Status::SubmissionPending);

        let artisan_bal_before = ctx.token_client.balance(&artisan);

        // Client approves and releases from SubmissionPending state
        ctx.client_contract.release(&id, &ctx.token_address);

        let artisan_bal_after = ctx.token_client.balance(&artisan);
        assert_eq!(artisan_bal_after, artisan_bal_before + 5_000);
        assert_eq!(ctx.get_escrow(id).status, Status::Released);

        // Storage key cleaned up
        ctx.env.as_contract(&ctx.contract_id, || {
            let stored_time: Option<u64> = ctx.env.storage().persistent().get(&DataKey::SubmissionPending(id));
            assert!(stored_time.is_none());
        });
    }

    #[test]
    fn test_dispute_allowed_from_submission_pending() {
        let ctx = TestContext::new();
        let (client, _artisan, id) = ctx.setup_funded_escrow(1_000, 4_000, 86_400);

        ctx.client_contract.submit_work(&id);
        assert_eq!(ctx.get_escrow(id).status, Status::SubmissionPending);

        ctx.client_contract.dispute(&id, &client);
        assert_eq!(ctx.get_escrow(id).status, Status::Disputed);
    }

    #[test]
    fn test_release_materials_allowed_from_submission_pending() {
        let ctx = TestContext::new();
        let (_client, artisan, id) = ctx.setup_funded_escrow(2_000, 3_000, 86_400);

        ctx.client_contract.submit_work(&id);
        assert_eq!(ctx.get_escrow(id).status, Status::SubmissionPending);

        let artisan_bal_before = ctx.token_client.balance(&artisan);
        ctx.client_contract.release_materials(&id, &ctx.token_address, &artisan);
        let artisan_bal_after = ctx.token_client.balance(&artisan);

        assert_eq!(artisan_bal_after, artisan_bal_before + 2_000);
        assert_eq!(ctx.get_escrow(id).status, Status::SubmissionPending);
        assert!(ctx.get_escrow(id).materials_released);
    }
}

