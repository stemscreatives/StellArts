#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, token, vec, Address, Env, Symbol, Vec};
// Protocol fee cap — 1000 bps = 10%. Prevents admin from misconfiguring
// an unreasonably high fee; adjust if product requirements change.
const MAX_FEE_BPS: u32 = 1_000;

// Max share of the protocol fee that may be allocated to jury rewards,
// out of 10_000 bps. 100% of the fee may be routed to the jury pool.
const MAX_JURY_REWARD_BPS: u32 = 10_000;

// TTL constants for persistent storage (in ledgers)
// Note: Each ledger is approximately 5 seconds
const ESCROW_TTL: u32 = 1_036_800; // ~60 days
const NEXT_ID_TTL: u32 = 6_220_800; // ~1 year
const TTL_THRESHOLD: u32 = 17_280; // ~1 day - triggers extension when TTL drops below this
pub const GRACE_PERIOD: u64 = 86_400; // 24 hours in seconds - artisan protection window after deadline
pub const SUBMISSION_GRACE_PERIOD: u64 = 43_200; // 12 hours in seconds - additional lock when Submission Pending is active

/// Multi-sig configuration for large escrow payments.
/// When `required_signers` is non-empty, `release` requires that at least
/// `threshold` of those signers have called `multisig_approve` before funds
/// can be transferred to the artisan.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiSigConfig {
    /// Addresses that are allowed to sign off on the release.
    pub required_signers: Vec<Address>,
    /// Number of approvals needed before release is permitted.
    pub threshold: u32,
}

/// Protocol fee configuration. `fee_basis_points` is out of 10_000
/// (e.g. 250 = 2.5%). Fee is only applied to artisan payouts, never
/// to client refunds.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreasuryConfig {
    pub treasury_address: Address,
    pub fee_basis_points: u32,
}

/// Tracks which signers have already approved a multi-sig release.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiSigApprovals {
    pub approvals: Vec<Address>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Escrow {
    pub client: Address,
    pub artisan: Address,
    pub arbitrator: Address,
    pub token: Address,
    pub material_amount: i128,
    pub labor_amount: i128,
    pub status: Status,
    pub deadline: u64,
    pub materials_released: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadlineExtension {
    pub new_deadline: u64,
    pub proposer: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    Pending,
    Funded,
    InProgress,
    Released,
    Refunded, // added for reclaimed/returned escrows
    Disputed,
    Resolved, // dispute resolved with split distribution
    SubmissionPending, // artisan submitted deliverables; locks reclaim for +12h
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EarlyReclaimApproval {
    pub proposer: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Escrow(u64),
    DeadlineExtension(u64),
    EarlyReclaim(u64),
    MultiSigConfig(u64),
    MultiSigApprovals(u64),
    /// Ordered milestone percentages (must sum to 100 when present).
    Milestones(u64),
    /// Index of the next milestone eligible for release.
    NextMilestone(u64),
    /// Cumulative amount already paid out via `release_milestone`.
    MilestoneReleased(u64),
    NextId,
    Oracle,
    Admin,
    IsPaused,
    Lock,
    Treasury,
    /// DAO dispute resolution contract authorized to resolve disputed escrows
    /// on behalf of a selected jury (`resolve_dispute_dao`).
    DisputeResolver,
    /// Portion of the protocol fee (out of 10_000) allocated to jury rewards.
    JuryRewardBps,
    /// Timestamp when SubmissionPending state was triggered by the artisan
    SubmissionPending(u64),
}

#[contracttype]
pub struct EngagementInitializedEvent {
    pub id: u64,
    pub client: Address,
    pub artisan: Address,
    pub arbitrator: Address,
    pub token: Address,
    pub material_amount: i128,
    pub labor_amount: i128,
}

#[contracttype]
pub struct FundsDepositedEvent {
    pub id: u64,
    pub client: Address,
    pub amount: i128,
    pub token: Address,
}

#[contracttype]
pub struct FundsReleasedEvent {
    pub id: u64,
    pub client: Address,
    pub artisan: Address,
    pub amount: i128,
    pub token: Address,
}

#[contracttype]
pub struct MaterialsReleasedEvent {
    pub id: u64,
    pub client: Address,
    pub artisan: Address,
    pub amount: i128,
    pub token: Address,
}

#[contracttype]
pub struct MilestoneReleasedEvent {
    pub id: u64,
    pub client: Address,
    pub artisan: Address,
    pub milestone_index: u32,
    pub percentage: u32,
    pub amount: i128,
    pub token: Address,
}

// Event emitted when a funded escrow is reclaimed by the client after the deadline
#[contracttype]
pub struct ReclaimedEvent {
    pub id: u64,
    pub client: Address,
    pub artisan: Address,
    pub amount: i128,
    pub token: Address,
    pub timestamp: u64,
}

#[contracttype]
pub struct TreasuryInitializedEvent {
    pub treasury: Address,
    pub fee_basis_points: u32,
}

#[contracttype]
pub struct FeeCollectedEvent {
    pub id: u64,
    pub treasury: Address,
    pub fee_amount: i128,
    pub token: Address,
}

// Event emitted when a dispute is initiated on an escrow
#[contracttype]
pub struct DisputeInitiatedEvent {
    pub id: u64,
    pub client: Address,
    pub artisan: Address,
    pub amount: i128,
    pub token: Address,
    pub initiator: Address,
    pub timestamp: u64,
}

// Event emitted when an arbitrator resolves a dispute
#[contracttype]
pub struct DisputeResolvedEvent {
    pub id: u64,
    pub client: Address,
    pub artisan: Address,
    pub token: Address,
    pub client_amount: i128,
    pub artisan_amount: i128,
    pub timestamp: u64,
}

// Event emitted when artisan transitions an escrow into Submission Pending
#[contracttype]
pub struct SubmissionPendingEvent {
    pub id: u64,
    pub artisan: Address,
    pub timestamp: u64,
}

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    fn check_and_set_lock(env: &Env) {
        let is_locked: bool = env
            .storage()
            .instance()
            .get(&DataKey::Lock)
            .unwrap_or(false);
        if is_locked {
            panic!("Reentrancy detected");
        }
        env.storage().instance().set(&DataKey::Lock, &true);
    }

    fn clear_lock(env: &Env) {
        env.storage().instance().set(&DataKey::Lock, &false);
    }

    pub fn init_admin(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Admin already set");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    pub fn is_paused(env: &Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::IsPaused)
            .unwrap_or(false)
    }

    pub fn pause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &true);
    }

    /// amount * bps / 10_000, floored. Floor rounding guarantees
    /// fee + remainder == amount exactly — no dust is ever left behind.
    fn calculate_fee(amount: i128, fee_bps: u32) -> i128 {
        amount
            .checked_mul(fee_bps as i128)
            .expect("fee calculation overflow")
            / 10_000
    }

    fn get_treasury_config(env: &Env) -> Option<TreasuryConfig> {
        env.storage().persistent().get(&DataKey::Treasury)
    }

    fn has_milestones(env: &Env, engagement_id: u64) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Milestones(engagement_id))
    }

    fn validate_milestones(milestones: &Vec<u32>) {
        if milestones.is_empty() {
            return;
        }
        let mut total: u32 = 0;
        for pct in milestones.iter() {
            if pct == 0 {
                panic!("Milestone percentages must be greater than zero");
            }
            total = total
                .checked_add(pct)
                .unwrap_or_else(|| panic!("Milestone percentage overflow"));
        }
        if total != 100 {
            panic!("Milestone percentages must sum to exactly 100");
        }
    }

    /// Remaining tokens still held by the contract for this engagement.
    fn remaining_escrow_balance(env: &Env, engagement_id: u64, escrow: &Escrow) -> i128 {
        let total = escrow.material_amount + escrow.labor_amount;
        if Self::has_milestones(env, engagement_id) {
            let released: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::MilestoneReleased(engagement_id))
                .unwrap_or(0);
            return total - released;
        }
        if escrow.materials_released {
            escrow.labor_amount
        } else {
            total
        }
    }

    fn pay_artisan_with_optional_fee(
        env: &Env,
        engagement_id: u64,
        escrow: &Escrow,
        token: &Address,
        transfer_amount: i128,
    ) {
        let token_client = token::Client::new(env, token);
        if let Some(cfg) = Self::get_treasury_config(env) {
            let fee = Self::calculate_fee(transfer_amount, cfg.fee_basis_points);
            let artisan_payout = transfer_amount - fee;

            if fee > 0 {
                token_client.transfer(&env.current_contract_address(), &cfg.treasury_address, &fee);
                env.events().publish(
                    (Symbol::new(env, "fee_collected"), engagement_id),
                    FeeCollectedEvent {
                        id: engagement_id,
                        treasury: cfg.treasury_address,
                        fee_amount: fee,
                        token: escrow.token.clone(),
                    },
                );
            }
            token_client.transfer(
                &env.current_contract_address(),
                &escrow.artisan,
                &artisan_payout,
            );
        } else {
            token_client.transfer(
                &env.current_contract_address(),
                &escrow.artisan,
                &transfer_amount,
            );
        }
    }

    pub fn unpause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &false);
    }

    /// Read a single engagement by id so other contracts can verify escrow state.
    pub fn get_engagement(env: Env, engagement_id: u64) -> Escrow {
        env.storage()
            .persistent()
            .get(&DataKey::Escrow(engagement_id))
            .expect("Escrow not found")
    }

    /// Initialize a new escrow engagement
    /// Creates a new escrow record with Pending status and a per-escrow arbitrator.
    ///
    /// Pass `multisig_signers` as a non-empty list and `multisig_threshold > 0` to
    /// enable multi-sig release for high-value jobs.  When multi-sig is enabled,
    /// `release` will require that at least `multisig_threshold` of the listed
    /// signers have called `multisig_approve` before funds are transferred.
    ///
    /// Pass `milestones` as percentages of the total escrow (e.g. `[25, 25, 50]`).
    /// When non-empty they must sum to exactly 100 and funds are unlocked via
    /// `release_milestone`. An empty list keeps the legacy all-or-nothing `release` flow.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        env: Env,
        client: Address,
        artisan: Address,
        arbitrator: Address,
        token: Address,
        material_amount: i128,
        labor_amount: i128,
        deadline: u64,
        multisig_signers: Vec<Address>,
        multisig_threshold: u32,
        milestones: Vec<u32>,
    ) -> u64 {
        assert!(!Self::is_paused(&env), "contract is paused");
        // Validation: client cannot be the same as artisan
        if client == artisan {
            panic!("Client and artisan cannot be the same address");
        }

        // Validation: arbitrator cannot be client or artisan
        if arbitrator == client || arbitrator == artisan {
            panic!("Arbitrator must be a third party");
        }

        if material_amount < 0 || labor_amount < 0 {
            panic!("Amounts must be non-negative");
        }

        let total = material_amount + labor_amount;
        if total <= 0 {
            panic!("Total amount must be greater than zero");
        }

        Self::validate_milestones(&milestones);

        // Multi-sig validation
        let multisig: Option<MultiSigConfig> = if multisig_signers.is_empty() {
            None
        } else if multisig_threshold == 0 || multisig_threshold > multisig_signers.len() {
            panic!("multisig_threshold must be between 1 and the number of signers");
        } else {
            Some(MultiSigConfig {
                required_signers: multisig_signers,
                threshold: multisig_threshold,
            })
        };

        // Get and increment the next engagement ID
        let id_key = DataKey::NextId;
        let engagement_id: u64 = env.storage().persistent().get(&id_key).unwrap_or(1);
        env.storage()
            .persistent()
            .set(&id_key, &(engagement_id + 1));

        // Store multi-sig config separately (Option<contracttype> is not supported by #[contracttype])
        if let Some(ref cfg) = multisig {
            let cfg_key = DataKey::MultiSigConfig(engagement_id);
            env.storage().persistent().set(&cfg_key, cfg);
            env.storage()
                .persistent()
                .extend_ttl(&cfg_key, TTL_THRESHOLD, ESCROW_TTL);
        }

        if !milestones.is_empty() {
            let milestones_key = DataKey::Milestones(engagement_id);
            env.storage().persistent().set(&milestones_key, &milestones);
            env.storage()
                .persistent()
                .extend_ttl(&milestones_key, TTL_THRESHOLD, ESCROW_TTL);

            let next_key = DataKey::NextMilestone(engagement_id);
            env.storage().persistent().set(&next_key, &0u32);
            env.storage()
                .persistent()
                .extend_ttl(&next_key, TTL_THRESHOLD, ESCROW_TTL);

            let released_key = DataKey::MilestoneReleased(engagement_id);
            env.storage().persistent().set(&released_key, &0i128);
            env.storage()
                .persistent()
                .extend_ttl(&released_key, TTL_THRESHOLD, ESCROW_TTL);
        }

        // Create the escrow record
        let escrow = Escrow {
            client: client.clone(),
            artisan: artisan.clone(),
            arbitrator: arbitrator.clone(),
            token: token.clone(),
            material_amount,
            labor_amount,
            status: Status::Pending,
            deadline,
            materials_released: false,
        };

        // Store the escrow record
        let escrow_key = DataKey::Escrow(engagement_id);
        env.storage().persistent().set(&escrow_key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&escrow_key, TTL_THRESHOLD, ESCROW_TTL);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "initialize"), engagement_id),
            EngagementInitializedEvent {
                id: engagement_id,
                client,
                artisan,
                arbitrator,
                token,
                material_amount,
                labor_amount,
            },
        );

        engagement_id
    }

    /// Deposit funds into escrow for a specific engagement
    /// The client must have previously authorized the escrow contract to spend tokens
    pub fn deposit(env: Env, engagement_id: u64, token: Address) {
        assert!(!Self::is_paused(&env), "contract is paused");
        // Load the escrow record
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic!("Escrow not found for engagement {}", engagement_id));

        // Verify token matches initialized token
        if token != escrow.token {
            panic!("Token does not match the initialized token for this engagement");
        }

        // Deadline enforcement: cannot deposit after the deadline has passed
        let current_time = env.ledger().timestamp();
        if current_time > escrow.deadline {
            panic!("Deadline has passed; cannot deposit into this escrow");
        }

        // Note: Authorization should be verified by the calling application
        // In a production system, this would require client signature verification
        // Auth: Require the client's signature
        escrow.client.require_auth();

        // Verify that the escrow is in Pending status
        if escrow.status != Status::Pending {
            panic!("Escrow must be in Pending status to deposit funds");
        }

        // Transfer tokens from client to escrow contract
        let total = escrow.material_amount + escrow.labor_amount;
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&escrow.client, &env.current_contract_address(), &total);

        // Update escrow status to Funded
        escrow.status = Status::Funded;

        // Save the updated escrow and extend TTL
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        // Emit event
        let total = escrow.material_amount + escrow.labor_amount;
        env.events().publish(
            (Symbol::new(&env, "deposit"), engagement_id),
            FundsDepositedEvent {
                id: engagement_id,
                client: escrow.client,
                amount: total,
                token: escrow.token,
            },
        );
    }

    /// Release material funds to the artisan immediately.
    /// Transfers only the material_amount to the artisan so they can purchase parts.
    /// This can be called by either the client or the artisan, and only once per engagement.
    /// After this call, the labor_amount remains locked in the contract until the job is done.
    pub fn release_materials(env: Env, engagement_id: u64, token: Address, caller: Address) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        // Verify token matches initialized token
        if token != escrow.token {
            panic!("Token does not match the initialized token for this engagement");
        }

        // Auth: either the client or the artisan may trigger material release
        caller.require_auth();
        if caller != escrow.client && caller != escrow.artisan {
            panic!("Only client or artisan can release materials");
        }

        // State check: must be funded, in progress, or submission pending
        if escrow.status != Status::Funded && escrow.status != Status::InProgress && escrow.status != Status::SubmissionPending {
            panic!("Escrow must be Funded or InProgress to release materials");
        }

        // Guard: prevent double-spending of material funds
        if escrow.materials_released {
            panic!("Materials have already been released for this engagement");
        }

        if Self::has_milestones(&env, engagement_id) {
            panic!("Use release_milestone for milestone-based escrows");
        }

        // Transfer material_amount to artisan
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(
            &env.current_contract_address(),
            &escrow.artisan,
            &escrow.material_amount,
        );

        // Mark materials as released
        escrow.materials_released = true;
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "release_materials"), engagement_id),
            MaterialsReleasedEvent {
                id: engagement_id,
                client: escrow.client.clone(),
                artisan: escrow.artisan.clone(),
                amount: escrow.material_amount,
                token: escrow.token.clone(),
            },
        );
    }

    /// Release funds from escrow to the artisan
    /// Can only be called by the client and only when escrow is funded.
    /// Also verifies that the deadline has not passed; after the deadline the client
    /// must use `reclaim` to retrieve funds instead.
    ///
    /// When the escrow was created with multi-sig enabled, the required number of
    /// signers must have already called `multisig_approve` before this succeeds.
    pub fn release(env: Env, engagement_id: u64, token: Address) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        if token != escrow.token {
            panic!("Token does not match the initialized token for this engagement");
        }

        escrow.client.require_auth();

        let current_time = env.ledger().timestamp();
        if current_time > escrow.deadline {
            panic!("Deadline has passed; cannot release funds");
        }

        if escrow.status != Status::Funded && escrow.status != Status::InProgress && escrow.status != Status::SubmissionPending {
            panic!("Escrow is not funded or in progress");
        }

        if Self::has_milestones(&env, engagement_id) {
            panic!("Use release_milestone for milestone-based escrows");
        }

        let cfg_key = DataKey::MultiSigConfig(engagement_id);
        if let Some(cfg) = env
            .storage()
            .persistent()
            .get::<DataKey, MultiSigConfig>(&cfg_key)
        {
            let approvals_key = DataKey::MultiSigApprovals(engagement_id);
            let approvals: MultiSigApprovals = env
                .storage()
                .persistent()
                .get(&approvals_key)
                .unwrap_or(MultiSigApprovals {
                    approvals: vec![&env],
                });
            if approvals.approvals.len() < cfg.threshold {
                panic!("Multi-sig threshold not met; more approvals required before release");
            }
            env.storage().persistent().remove(&approvals_key);
        }

        Self::check_and_set_lock(&env);

        let transfer_amount = if escrow.materials_released {
            escrow.labor_amount
        } else {
            escrow.material_amount + escrow.labor_amount
        };

        Self::pay_artisan_with_optional_fee(&env, engagement_id, &escrow, &token, transfer_amount);

        escrow.status = Status::Released;
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .remove(&DataKey::SubmissionPending(engagement_id));
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        env.events().publish(
            (Symbol::new(&env, "release"), engagement_id),
            FundsReleasedEvent {
                id: engagement_id,
                client: escrow.client.clone(),
                artisan: escrow.artisan.clone(),
                amount: transfer_amount,
                token: escrow.token.clone(),
            },
        );

        Self::clear_lock(&env);
    }

    /// Release funds for the current active milestone.
    ///
    /// Milestones must be released in order (index 0, then 1, …). The final
    /// milestone pays any remainder so rounding never leaves dust in the contract.
    /// After the last milestone is paid, status becomes `Released`.
    pub fn release_milestone(env: Env, engagement_id: u64, token: Address) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        if token != escrow.token {
            panic!("Token does not match the initialized token for this engagement");
        }

        escrow.client.require_auth();

        let current_time = env.ledger().timestamp();
        if current_time > escrow.deadline {
            panic!("Deadline has passed; cannot release funds");
        }

        let milestones_key = DataKey::Milestones(engagement_id);
        let milestones: Vec<u32> = env
            .storage()
            .persistent()
            .get(&milestones_key)
            .unwrap_or_else(|| panic!("Escrow has no milestones configured"));

        let next_key = DataKey::NextMilestone(engagement_id);
        let next_index: u32 = env.storage().persistent().get(&next_key).unwrap_or(0);

        if next_index >= milestones.len() {
            panic!("All milestones have already been released");
        }

        if escrow.status != Status::Funded && escrow.status != Status::InProgress && escrow.status != Status::SubmissionPending {
            panic!("Escrow is not funded or in progress");
        }

        let cfg_key = DataKey::MultiSigConfig(engagement_id);
        if let Some(cfg) = env
            .storage()
            .persistent()
            .get::<DataKey, MultiSigConfig>(&cfg_key)
        {
            let approvals_key = DataKey::MultiSigApprovals(engagement_id);
            let approvals: MultiSigApprovals = env
                .storage()
                .persistent()
                .get(&approvals_key)
                .unwrap_or(MultiSigApprovals {
                    approvals: vec![&env],
                });
            if approvals.approvals.len() < cfg.threshold {
                panic!("Multi-sig threshold not met; more approvals required before release");
            }
            // Approvals gate the full payout sequence; clear once the last milestone pays.
            if next_index + 1 >= milestones.len() {
                env.storage().persistent().remove(&approvals_key);
            }
        }

        Self::check_and_set_lock(&env);

        let total = escrow.material_amount + escrow.labor_amount;
        let released_key = DataKey::MilestoneReleased(engagement_id);
        let already_released: i128 = env.storage().persistent().get(&released_key).unwrap_or(0);

        let percentage = milestones.get(next_index).unwrap();
        let is_last = next_index + 1 >= milestones.len();
        let transfer_amount = if is_last {
            total - already_released
        } else {
            total
                .checked_mul(percentage as i128)
                .unwrap_or_else(|| panic!("milestone amount overflow"))
                / 100
        };

        if transfer_amount <= 0 {
            panic!("Milestone payout must be positive");
        }

        Self::pay_artisan_with_optional_fee(&env, engagement_id, &escrow, &token, transfer_amount);

        let new_released = already_released + transfer_amount;
        env.storage().persistent().set(&released_key, &new_released);
        env.storage()
            .persistent()
            .extend_ttl(&released_key, TTL_THRESHOLD, ESCROW_TTL);

        let new_index = next_index + 1;
        env.storage().persistent().set(&next_key, &new_index);
        env.storage()
            .persistent()
            .extend_ttl(&next_key, TTL_THRESHOLD, ESCROW_TTL);
        env.storage()
            .persistent()
            .extend_ttl(&milestones_key, TTL_THRESHOLD, ESCROW_TTL);

        if is_last {
            escrow.status = Status::Released;
        }

        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        env.events().publish(
            (Symbol::new(&env, "release_milestone"), engagement_id),
            MilestoneReleasedEvent {
                id: engagement_id,
                client: escrow.client.clone(),
                artisan: escrow.artisan.clone(),
                milestone_index: next_index,
                percentage,
                amount: transfer_amount,
                token: escrow.token.clone(),
            },
        );

        Self::clear_lock(&env);
    }

    /// Milestone percentages configured for an engagement (empty if none).
    pub fn get_milestones(env: Env, engagement_id: u64) -> Vec<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::Milestones(engagement_id))
            .unwrap_or(vec![&env])
    }

    /// Index of the next milestone to release (equal to milestone count when finished).
    pub fn get_next_milestone(env: Env, engagement_id: u64) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::NextMilestone(engagement_id))
            .unwrap_or(0)
    }

    /// Record a signer's approval for a multi-sig escrow release.
    ///
    /// Only addresses listed in the escrow's `multisig.required_signers` may call
    /// this.  Each signer may only approve once.  Once the threshold is reached,
    /// the client can call `release` to transfer funds.
    pub fn multisig_approve(env: Env, engagement_id: u64, signer: Address) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        signer.require_auth();

        let cfg: MultiSigConfig = env
            .storage()
            .persistent()
            .get(&DataKey::MultiSigConfig(engagement_id))
            .expect("Escrow does not have multi-sig enabled");

        // Verify signer is in the required list
        let mut is_allowed = false;
        for s in cfg.required_signers.iter() {
            if s == signer {
                is_allowed = true;
                break;
            }
        }
        if !is_allowed {
            panic!("Signer is not in the multi-sig required signers list");
        }

        // Escrow must be Funded, InProgress, or SubmissionPending
        if escrow.status != Status::Funded && escrow.status != Status::InProgress && escrow.status != Status::SubmissionPending {
            panic!("Escrow must be Funded or InProgress to approve");
        }

        let approvals_key = DataKey::MultiSigApprovals(engagement_id);
        let mut approvals: MultiSigApprovals = env
            .storage()
            .persistent()
            .get(&approvals_key)
            .unwrap_or(MultiSigApprovals {
                approvals: vec![&env],
            });

        // Prevent duplicate approvals
        for existing in approvals.approvals.iter() {
            if existing == signer {
                panic!("Signer has already approved this escrow");
            }
        }

        approvals.approvals.push_back(signer);
        env.storage().persistent().set(&approvals_key, &approvals);
        env.storage()
            .persistent()
            .extend_ttl(&approvals_key, TTL_THRESHOLD, ESCROW_TTL);
    }

    /// Allow the client to reclaim funds after the deadline + grace period has passed,
    /// or after the deadline if both parties have mutually approved an early reclaim.
    ///
    /// The grace period (24 hours) protects artisans from malicious reclaims when work
    /// is nearly finished. Both parties can bypass it via `approve_early_reclaim()`.
    ///
    /// Transfers the amount back to the client, updates the status to `Refunded`, and emits a
    /// [`ReclaimedEvent`]. Returns `true` on success.
    pub fn reclaim(env: Env, engagement_id: u64, token: Address) -> bool {
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        // Auth: only the client may reclaim
        escrow.client.require_auth();

        // State check: must be funded, in progress, or submission pending
        if escrow.status != Status::Funded
            && escrow.status != Status::InProgress
            && escrow.status != Status::SubmissionPending
        {
            panic!("Escrow must be Funded to reclaim");
        }

        // Deadline + grace period check
        let current_time = env.ledger().timestamp();
        if current_time <= escrow.deadline {
            panic!("Deadline has not passed; cannot reclaim yet");
        }

        // Base grace period check
        let base_grace_deadline = escrow.deadline + GRACE_PERIOD;
        if escrow.status == Status::SubmissionPending {
            let sub_key = DataKey::SubmissionPending(engagement_id);
            let submitted_at: u64 = env
                .storage()
                .persistent()
                .get(&sub_key)
                .unwrap_or(current_time);

            // Additional 12 hours lock: client cannot reclaim during the extended window
            let submission_locked_until = core::cmp::max(
                base_grace_deadline + SUBMISSION_GRACE_PERIOD,
                submitted_at + SUBMISSION_GRACE_PERIOD,
            );

            if current_time <= submission_locked_until {
                panic!("Submission Pending is active; reclaim is locked for an additional 12 hours");
            }

            // Cleanup submission pending storage key
            env.storage().persistent().remove(&sub_key);
        } else if current_time <= base_grace_deadline {
            // Within grace period: only allow if both parties approved early reclaim
            let early_reclaim_key = DataKey::EarlyReclaim(engagement_id);
            let has_early_approval: bool = env
                .storage()
                .persistent()
                .get::<DataKey, bool>(&early_reclaim_key)
                .unwrap_or(false);

            if !has_early_approval {
                panic!("Grace period has not passed; both parties must approve early reclaim");
            }

            // Clean up the early reclaim approval
            env.storage().persistent().remove(&early_reclaim_key);
        }

        // Verify token matches initialized token
        if token != escrow.token {
            panic!("Token does not match the initialized token for this engagement");
        }

        Self::check_and_set_lock(&env);
        // Refund whatever is still locked (accounts for materials or milestones).
        let refund_amount = Self::remaining_escrow_balance(&env, engagement_id, &escrow);
        if refund_amount <= 0 {
            panic!("No funds remaining to reclaim");
        }

        // Transfer funds back to the client
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(
            &env.current_contract_address(),
            &escrow.client,
            &refund_amount,
        );

        // Update state to Refunded
        escrow.status = Status::Refunded;
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        let current_time = env.ledger().timestamp();
        // Emit event
        env.events().publish(
            (Symbol::new(&env, "reclaim"), engagement_id),
            ReclaimedEvent {
                id: engagement_id,
                client: escrow.client.clone(),
                artisan: escrow.artisan.clone(),
                amount: refund_amount,
                token: escrow.token.clone(),
                timestamp: current_time,
            },
        );

        Self::clear_lock(&env);

        true
    }

    /// Mutually approve an early reclaim during the grace period.
    /// Both client and artisan must call this function to approve.
    /// Once both approve, the client can call `reclaim()` during the grace period.
    pub fn approve_early_reclaim(env: Env, engagement_id: u64, approver: Address) {
        let key = DataKey::Escrow(engagement_id);
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        approver.require_auth();

        // Only client or artisan can approve
        if approver != escrow.client && approver != escrow.artisan {
            panic!("Only client or artisan can approve early reclaim");
        }

        // State check: must be funded, in progress, or submission pending
        if escrow.status != Status::Funded
            && escrow.status != Status::InProgress
            && escrow.status != Status::SubmissionPending
        {
            panic!("Escrow must be Funded to approve early reclaim");
        }

        let early_reclaim_key = DataKey::EarlyReclaim(engagement_id);
        let pending: Option<EarlyReclaimApproval> =
            env.storage().persistent().get(&early_reclaim_key);

        if let Some(approval) = pending {
            // Second approver — must be different party
            if approval.proposer == approver {
                panic!("Same party cannot approve early reclaim twice");
            }

            // Both parties agree: store a simple bool flag
            env.storage().persistent().set(&early_reclaim_key, &true);
            env.storage()
                .persistent()
                .extend_ttl(&early_reclaim_key, TTL_THRESHOLD, ESCROW_TTL);
        } else {
            // First approver — store the proposal
            let approval = EarlyReclaimApproval { proposer: approver };
            env.storage()
                .persistent()
                .set(&early_reclaim_key, &approval);
            env.storage()
                .persistent()
                .extend_ttl(&early_reclaim_key, TTL_THRESHOLD, ESCROW_TTL);
        }
    }

    /// Mutually approve and apply an escrow deadline extension.
    pub fn extend_deadline(env: Env, engagement_id: u64, approver: Address, new_deadline: u64) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        approver.require_auth();

        if approver != escrow.client && approver != escrow.artisan {
            panic!("Only client or artisan can approve deadline extension");
        }

        if escrow.status != Status::Funded
            && escrow.status != Status::InProgress
            && escrow.status != Status::SubmissionPending
        {
            panic!("Escrow must be Funded or InProgress to extend deadline");
        }

        if new_deadline <= escrow.deadline {
            panic!("New deadline must be greater than current deadline");
        }

        let extension_key = DataKey::DeadlineExtension(engagement_id);
        let pending: Option<DeadlineExtension> = env.storage().persistent().get(&extension_key);

        if let Some(extension) = pending {
            if extension.proposer == approver {
                panic!("Same party cannot approve deadline extension twice");
            }

            if extension.new_deadline != new_deadline {
                panic!("Pending deadline extension does not match requested deadline");
            }

            escrow.deadline = new_deadline;
            env.storage().persistent().set(&key, &escrow);
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);
            env.storage().persistent().remove(&extension_key);
        } else {
            let extension = DeadlineExtension {
                new_deadline,
                proposer: approver,
            };
            env.storage().persistent().set(&extension_key, &extension);
            env.storage()
                .persistent()
                .extend_ttl(&extension_key, TTL_THRESHOLD, ESCROW_TTL);
        }
    }

    /// Set the oracle address for verifying physical arrival
    pub fn set_oracle(env: Env, admin: Address, oracle: Address) {
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Oracle, &oracle);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::Oracle, TTL_THRESHOLD, NEXT_ID_TTL);
    }

    /// Configure (or update) the protocol treasury and fee rate.
    /// Only the admin set via `init_admin` can call this.
    pub fn init_treasury(
        env: Env,
        admin: Address,
        treasury_address: Address,
        fee_basis_points: u32,
    ) {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic!("Only admin can configure treasury");
        }
        admin.require_auth();

        if fee_basis_points > MAX_FEE_BPS {
            panic!("fee_basis_points exceeds maximum allowed (10%)");
        }

        let cfg = TreasuryConfig {
            treasury_address: treasury_address.clone(),
            fee_basis_points,
        };
        env.storage().persistent().set(&DataKey::Treasury, &cfg);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::Treasury, TTL_THRESHOLD, NEXT_ID_TTL);

        env.events().publish(
            (Symbol::new(&env, "treasury_init"),),
            TreasuryInitializedEvent {
                treasury: treasury_address,
                fee_basis_points,
            },
        );
    }

    /// Read the current treasury configuration, if any has been set.
    pub fn get_treasury(env: Env) -> Option<TreasuryConfig> {
        Self::get_treasury_config(&env)
    }

    /// Register the DAO dispute resolution contract that is allowed to call
    /// `resolve_dispute_dao` to execute jury verdicts on disputed escrows.
    /// Only the admin set via `init_admin` can call this.
    pub fn set_dispute_resolver(env: Env, admin: Address, resolver: Address) {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic!("Only admin can configure the dispute resolver");
        }
        admin.require_auth();

        env.storage()
            .persistent()
            .set(&DataKey::DisputeResolver, &resolver);
        env.storage().persistent().extend_ttl(
            &DataKey::DisputeResolver,
            TTL_THRESHOLD,
            NEXT_ID_TTL,
        );
    }

    /// Read the registered DAO dispute resolution contract, if any.
    pub fn get_dispute_resolver(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::DisputeResolver)
    }

    /// Configure the portion of the protocol fee (out of 10_000 bps) that is
    /// routed to the DAO dispute resolution contract as jury rewards when a
    /// dispute is resolved via `resolve_dispute_dao`. The remainder of the fee
    /// continues to flow to the treasury. Only the admin can call this.
    pub fn set_jury_reward_bps(env: Env, admin: Address, bps: u32) {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic!("Only admin can configure jury rewards");
        }
        admin.require_auth();

        if bps > MAX_JURY_REWARD_BPS {
            panic!("jury_reward_bps exceeds maximum allowed");
        }

        env.storage()
            .persistent()
            .set(&DataKey::JuryRewardBps, &bps);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::JuryRewardBps, TTL_THRESHOLD, NEXT_ID_TTL);
    }

    /// Read the configured jury reward share of the protocol fee (out of 10_000).
    pub fn get_jury_reward_bps(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::JuryRewardBps)
            .unwrap_or(0)
    }

    /// Tokens still locked in the contract for this engagement.
    /// Cross-contract helper so other contracts can compute dispute splits.
    pub fn get_remaining_balance(env: Env, engagement_id: u64) -> i128 {
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(engagement_id))
            .expect("Escrow not found");
        Self::remaining_escrow_balance(&env, engagement_id, &escrow)
    }

    /// Execute a jury verdict on a disputed escrow.
    ///
    /// Only the dispute resolution contract registered via `set_dispute_resolver`
    /// may call this. It mirrors `resolve_dispute` (same distribution invariant:
    /// `client_amount + artisan_amount == remaining balance`) but the protocol fee
    /// is split: `jury_reward_bps` of the fee is transferred to the resolver as
    /// the jury reward pool, and the rest stays with the treasury.
    ///
    /// Returns the jury reward amount transferred to the resolver (0 when no
    /// treasury/fee is configured), so the caller knows exactly how much to
    /// distribute to the majority jurors.
    pub fn resolve_dispute_dao(
        env: Env,
        engagement_id: u64,
        client_amount: i128,
        artisan_amount: i128,
        token: Address,
    ) -> i128 {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        let resolver: Address = env
            .storage()
            .persistent()
            .get(&DataKey::DisputeResolver)
            .expect("Dispute resolver not set");
        resolver.require_auth();

        if escrow.status != Status::Disputed {
            panic!("Escrow must be in Disputed status to resolve");
        }

        if client_amount < 0 || artisan_amount < 0 {
            panic!("Distribution amounts must be non-negative");
        }

        let remaining = Self::remaining_escrow_balance(&env, engagement_id, &escrow);
        if client_amount + artisan_amount != remaining {
            panic!("Distribution amounts must equal the remaining escrowed amount");
        }

        if token != escrow.token {
            panic!("Token does not match the initialized token for this engagement");
        }

        Self::check_and_set_lock(&env);

        if artisan_amount == 0 {
            escrow.status = Status::Refunded;
        } else if client_amount == 0 {
            escrow.status = Status::Released;
        } else {
            escrow.status = Status::Resolved;
        }

        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        let current_time = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "dispute_resolved"), engagement_id),
            DisputeResolvedEvent {
                id: engagement_id,
                client: escrow.client.clone(),
                artisan: escrow.artisan.clone(),
                token: escrow.token.clone(),
                client_amount,
                artisan_amount,
                timestamp: current_time,
            },
        );

        let token_client = token::Client::new(&env, &token);

        // Client refunds are never fee'd.
        if client_amount > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &escrow.client,
                &client_amount,
            );
        }

        let mut jury_reward: i128 = 0;

        // Protocol fee applies only to the artisan's share, proportionately.
        if artisan_amount > 0 {
            if let Some(cfg) = Self::get_treasury_config(&env) {
                let fee = Self::calculate_fee(artisan_amount, cfg.fee_basis_points);
                let artisan_payout = artisan_amount - fee;

                // Split the fee: the configured share funds the jury reward pool
                // (held by the dispute resolution contract); the rest is treasury.
                let reward_bps = Self::get_jury_reward_bps(env.clone());
                jury_reward = Self::calculate_fee(fee, reward_bps);
                let treasury_share = fee - jury_reward;

                if jury_reward > 0 {
                    token_client.transfer(&env.current_contract_address(), &resolver, &jury_reward);
                }

                if treasury_share > 0 {
                    token_client.transfer(
                        &env.current_contract_address(),
                        &cfg.treasury_address,
                        &treasury_share,
                    );
                    env.events().publish(
                        (Symbol::new(&env, "fee_collected"), engagement_id),
                        FeeCollectedEvent {
                            id: engagement_id,
                            treasury: cfg.treasury_address,
                            fee_amount: treasury_share,
                            token: escrow.token.clone(),
                        },
                    );
                }

                token_client.transfer(
                    &env.current_contract_address(),
                    &escrow.artisan,
                    &artisan_payout,
                );
            } else {
                token_client.transfer(
                    &env.current_contract_address(),
                    &escrow.artisan,
                    &artisan_amount,
                );
            }
        }

        Self::clear_lock(&env);

        jury_reward
    }

    /// Transition escrow status from Funded to InProgress
    pub fn start_job(env: Env, engagement_id: u64) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let oracle: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Oracle)
            .expect("Oracle not set");
        oracle.require_auth();

        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        if escrow.status != Status::Funded {
            panic!("Escrow must be Funded to transition to InProgress");
        }

        escrow.status = Status::InProgress;
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);
    }

    /// Transitions escrow status to `SubmissionPending` when artisan submits deliverables.
    /// This locks the client's ability to call `reclaim` for an additional 12 hours
    /// beyond the normal grace period to prevent TOCTOU race condition sniping.
    pub fn submit_work(env: Env, engagement_id: u64) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        // Auth: strictly the artisan triggers submission pending
        escrow.artisan.require_auth();

        // Must be in Funded, InProgress, or already SubmissionPending
        if escrow.status != Status::Funded
            && escrow.status != Status::InProgress
            && escrow.status != Status::SubmissionPending
        {
            panic!("Escrow must be Funded or InProgress to submit work");
        }

        let current_time = env.ledger().timestamp();

        // Record submission timestamp
        let sub_key = DataKey::SubmissionPending(engagement_id);
        env.storage().persistent().set(&sub_key, &current_time);
        env.storage()
            .persistent()
            .extend_ttl(&sub_key, TTL_THRESHOLD, ESCROW_TTL);

        escrow.status = Status::SubmissionPending;
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "submit_work"), engagement_id),
            SubmissionPendingEvent {
                id: engagement_id,
                artisan: escrow.artisan.clone(),
                timestamp: current_time,
            },
        );
    }

    /// Alias for submit_work matching the exact issue acceptance criteria phrasing
    pub fn trigger_submission_pending(env: Env, engagement_id: u64) {
        Self::submit_work(env, engagement_id);
    }

    /// Initiate a dispute on a funded escrow
    /// Can be called by either the client or artisan
    /// Transitions the escrow from Funded to Disputed status
    pub fn dispute(env: Env, engagement_id: u64, initiator: Address) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        // Auth: Require initiator to authorize invocation
        initiator.require_auth();

        // Auth: Only client or artisan can initiate a dispute
        if initiator != escrow.client && initiator != escrow.artisan {
            panic!("Only client or artisan can initiate a dispute");
        }

        // State check: escrow must be Funded, InProgress, or SubmissionPending to dispute
        if escrow.status != Status::Funded
            && escrow.status != Status::InProgress
            && escrow.status != Status::SubmissionPending
        {
            panic!("Escrow must be Funded or InProgress to initiate a dispute");
        }

        // Transition status to Disputed
        escrow.status = Status::Disputed;
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        // Emit event
        let current_time = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "dispute_initiated"), engagement_id),
            DisputeInitiatedEvent {
                id: engagement_id,
                client: escrow.client.clone(),
                artisan: escrow.artisan.clone(),
                amount: escrow.material_amount + escrow.labor_amount,
                token: escrow.token.clone(),
                initiator,
                timestamp: current_time,
            },
        );
    }

    /// Resolve a dispute by the per-escrow arbitrator
    /// Only callable by the arbitrator assigned at escrow initialization
    /// Supports split distribution: funds can be divided between client and artisan
    /// The sum of client_amount and artisan_amount must equal the remaining escrowed amount
    pub fn resolve_dispute(
        env: Env,
        engagement_id: u64,
        client_amount: i128,
        artisan_amount: i128,
        token: Address,
    ) {
        assert!(!Self::is_paused(&env), "contract is paused");
        let key = DataKey::Escrow(engagement_id);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        escrow.arbitrator.require_auth();

        if escrow.status != Status::Disputed {
            panic!("Escrow must be in Disputed status to resolve");
        }

        if client_amount < 0 || artisan_amount < 0 {
            panic!("Distribution amounts must be non-negative");
        }

        let remaining = Self::remaining_escrow_balance(&env, engagement_id, &escrow);
        if client_amount + artisan_amount != remaining {
            panic!("Distribution amounts must equal the remaining escrowed amount");
        }

        if token != escrow.token {
            panic!("Token does not match the initialized token for this engagement");
        }

        Self::check_and_set_lock(&env);

        if artisan_amount == 0 {
            escrow.status = Status::Refunded;
        } else if client_amount == 0 {
            escrow.status = Status::Released;
        } else {
            escrow.status = Status::Resolved;
        }

        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, ESCROW_TTL);

        let current_time = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "dispute_resolved"), engagement_id),
            DisputeResolvedEvent {
                id: engagement_id,
                client: escrow.client.clone(),
                artisan: escrow.artisan.clone(),
                token: escrow.token.clone(),
                client_amount,
                artisan_amount,
                timestamp: current_time,
            },
        );

        let token_client = token::Client::new(&env, &token);

        // Client refunds are never fee'd.
        if client_amount > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &escrow.client,
                &client_amount,
            );
        }

        // Protocol fee applies only to the artisan's share, proportionately.
        if artisan_amount > 0 {
            if let Some(cfg) = Self::get_treasury_config(&env) {
                let fee = Self::calculate_fee(artisan_amount, cfg.fee_basis_points);
                let artisan_payout = artisan_amount - fee;

                if fee > 0 {
                    token_client.transfer(
                        &env.current_contract_address(),
                        &cfg.treasury_address,
                        &fee,
                    );
                    env.events().publish(
                        (Symbol::new(&env, "fee_collected"), engagement_id),
                        FeeCollectedEvent {
                            id: engagement_id,
                            treasury: cfg.treasury_address,
                            fee_amount: fee,
                            token: escrow.token.clone(),
                        },
                    );
                }
                token_client.transfer(
                    &env.current_contract_address(),
                    &escrow.artisan,
                    &artisan_payout,
                );
            } else {
                token_client.transfer(
                    &env.current_contract_address(),
                    &escrow.artisan,
                    &artisan_amount,
                );
            }
        }

        Self::clear_lock(&env);
    }

    /// Remove storage entries for a list of finalized escrow IDs.
    ///
    /// Only escrows in `Released`, `Refunded`, or `Resolved` status can be
    /// cleaned up.  The caller must be the client of each escrow (acts as an
    /// audit gate so only the paying party can trigger cleanup).
    ///
    /// Emits a `cleanup` event for each removed escrow so off-chain indexers can
    /// archive the data before it disappears from on-chain storage.
    pub fn cleanup_expired(env: Env, engagement_ids: Vec<u64>) {
        assert!(!Self::is_paused(&env), "contract is paused");
        for engagement_id in engagement_ids.iter() {
            let key = DataKey::Escrow(engagement_id);
            let escrow: Escrow = match env.storage().persistent().get(&key) {
                Some(e) => e,
                None => continue, // already removed – skip silently
            };

            // Only finalized escrows may be cleaned up
            match escrow.status {
                Status::Released | Status::Refunded | Status::Resolved => {}
                _ => panic!("Escrow {} is not in a finalized state", engagement_id),
            }

            // Auth: only the client may trigger cleanup
            escrow.client.require_auth();

            // Emit event before removal so indexers can archive the record
            env.events()
                .publish((Symbol::new(&env, "cleanup"), engagement_id), engagement_id);

            // Remove primary escrow entry
            env.storage().persistent().remove(&key);

            // Remove any leftover auxiliary entries (best-effort)
            let cfg_key = DataKey::MultiSigConfig(engagement_id);
            if env.storage().persistent().has(&cfg_key) {
                env.storage().persistent().remove(&cfg_key);
            }
            let approvals_key = DataKey::MultiSigApprovals(engagement_id);
            if env.storage().persistent().has(&approvals_key) {
                env.storage().persistent().remove(&approvals_key);
            }
            for aux in [
                DataKey::Milestones(engagement_id),
                DataKey::NextMilestone(engagement_id),
                DataKey::MilestoneReleased(engagement_id),
            ] {
                if env.storage().persistent().has(&aux) {
                    env.storage().persistent().remove(&aux);
                }
            }
        }
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_legacy {
    use super::*;
    use soroban_sdk::testutils::{Events, Ledger};
    use soroban_sdk::{testutils::Address as _, Address, Env, IntoVal};

    #[test]
    fn test_initialize_engagement() {
        let env = Env::default();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        // Create test addresses
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let arbitrator_address = Address::generate(&env);
        let token_address = Address::generate(&env);
        let amount: i128 = 1000;
        let deadline = env.ledger().timestamp() + 86400; // 24 hours from now

        // Initialize engagement
        let engagement_id = client.initialize(
            &client_address,
            &artisan_address,
            &arbitrator_address,
            &token_address,
            &amount,
            &0,
            &deadline,
            &soroban_sdk::vec![&env],
            &0u32,
            &soroban_sdk::vec![&env],
        );

        // Verify the returned ID is valid (should be 1 for first engagement)
        assert_eq!(engagement_id, 1);

        // Verify the escrow was stored correctly
        let stored_escrow: Escrow = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::Escrow(engagement_id))
                .unwrap()
        });

        assert_eq!(stored_escrow.client, client_address);
        assert_eq!(stored_escrow.artisan, artisan_address);
        assert_eq!(stored_escrow.token, token_address);
        assert_eq!(stored_escrow.material_amount, amount);
        assert_eq!(stored_escrow.labor_amount, 0);
        assert_eq!(stored_escrow.status, Status::Pending);
        assert_eq!(stored_escrow.deadline, deadline);
        assert!(!stored_escrow.materials_released);

        // Verify next ID was updated
        let next_id: u64 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::NextId)
                .unwrap_or(1)
        });
        assert_eq!(next_id, 2); // Should be incremented to 2
    }

    #[test]
    #[should_panic(expected = "Client and artisan cannot be the same address")]
    fn test_initialize_same_client_artisan() {
        let env = Env::default();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let same_address = Address::generate(&env);
        let arbitrator_address = Address::generate(&env);
        let token_address = Address::generate(&env);
        let amount: i128 = 1000;
        let deadline = env.ledger().timestamp() + 86400;

        // This should panic because client == artisan
        client.initialize(
            &same_address,
            &same_address,
            &arbitrator_address,
            &token_address,
            &amount,
            &0,
            &deadline,
            &soroban_sdk::vec![&env],
            &0u32,
            &soroban_sdk::vec![&env],
        );
    }

    #[test]
    #[should_panic(expected = "Total amount must be greater than zero")]
    fn test_initialize_zero_amount() {
        let env = Env::default();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let arbitrator_address = Address::generate(&env);
        let token_address = Address::generate(&env);
        let zero_amount: i128 = 0;
        let deadline = env.ledger().timestamp() + 86400;

        // This should panic because total amount is zero
        client.initialize(
            &client_address,
            &artisan_address,
            &arbitrator_address,
            &token_address,
            &zero_amount,
            &0,
            &deadline,
            &soroban_sdk::vec![&env],
            &0u32,
            &soroban_sdk::vec![&env],
        );
    }

    #[test]
    #[should_panic(expected = "Amounts must be non-negative")]
    fn test_initialize_negative_amount() {
        let env = Env::default();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let arbitrator_address = Address::generate(&env);
        let token_address = Address::generate(&env);
        let negative_amount: i128 = -100;
        let deadline = env.ledger().timestamp() + 86400;

        // This should panic because amount is negative
        client.initialize(
            &client_address,
            &artisan_address,
            &arbitrator_address,
            &token_address,
            &negative_amount,
            &0,
            &deadline,
            &soroban_sdk::vec![&env],
            &0u32,
            &soroban_sdk::vec![&env],
        );
    }

    // NOTE: This test fails due to Soroban token authentication complexity
    // The deposit function requires client approval for token transfers, which is
    // extremely complex to mock in unit tests. The core deposit logic is verified
    // by other tests that don't involve cross-contract token transfers.
    // In production, clients would approve the escrow contract before calling deposit.
    #[test]
    fn test_deposit_funds() {
        let env = Env::default();
        // Allow non-root auth for token transfer invoked by the escrow contract.
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register_contract(None, EscrowContract);
        let _client = EscrowContractClient::new(&env, &contract_id);

        // Create test addresses
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);

        // Create a mock token contract
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);

        // Mint some tokens to the client using the token contract's mint function
        let initial_amount: i128 = 1000;
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

        token_contract_client.mint(&client_address, &initial_amount);

        // Create an escrow record
        let engagement_id = 1;
        let escrow_amount: i128 = 500;
        let deadline = env.ledger().timestamp() + 86400; // 24 hours from now

        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: escrow_amount,
            labor_amount: 0,
            status: Status::Pending,
            deadline,
            materials_released: false,
        };

        // Store the escrow in contract storage
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        // Check initial token balance of contract (should be 0)
        let initial_contract_balance = token_client.balance(&contract_id);
        assert_eq!(initial_contract_balance, 0);

        // Call deposit function as the client
        EscrowContractClient::new(&env, &contract_id).deposit(&engagement_id, &token_address);

        // Verify contract's token balance increased
        let final_contract_balance = token_client.balance(&contract_id);
        assert_eq!(final_contract_balance, escrow_amount);

        // Verify escrow status was updated to Funded
        let updated_escrow: Escrow = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::Escrow(engagement_id))
                .unwrap()
        });
        assert_eq!(updated_escrow.status, Status::Funded);

        // Verify other fields remain unchanged
        assert_eq!(updated_escrow.client, client_address);
        assert_eq!(updated_escrow.material_amount, escrow_amount);
        assert_eq!(updated_escrow.labor_amount, 0);
        assert_eq!(updated_escrow.deadline, deadline);
        assert!(!updated_escrow.materials_released);
    }

    #[test]
    #[should_panic]
    fn test_deposit_unauthorized_client() {
        let env = Env::default();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        // Create test addresses
        let client_address = Address::generate(&env);
        let unauthorized_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);

        // Create a mock token and mint tokens to the client so the test
        // doesn't fail on insufficient balance before reaching the auth check
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);
        token_contract_client.mint(&client_address, &1000i128);

        // Create an escrow record owned by client_address
        let engagement_id = 1;
        let escrow_amount: i128 = 500;
        let deadline = env.ledger().timestamp() + 86400;

        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: escrow_amount,
            labor_amount: 0,
            status: Status::Pending,
            deadline,
            materials_released: false,
        };

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        // Mock auth for the WRONG address — unauthorized_address is not the client
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &unauthorized_address,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (engagement_id, token_address.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        // This must panic — unauthorized_address cannot satisfy escrow.client.require_auth()
        client.deposit(&engagement_id, &token_address);
    }

    #[test]
    #[should_panic(expected = "Escrow must be in Pending status to deposit funds")]
    fn test_deposit_already_funded() {
        let env = Env::default();
        let contract_id = env.register_contract(None, EscrowContract);
        let _client = EscrowContractClient::new(&env, &contract_id);

        // Create test addresses
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);

        // Create a mock token
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();

        // Create an escrow record that's already funded
        let engagement_id = 1;
        let escrow_amount: i128 = 500;
        let deadline = env.ledger().timestamp() + 86400;

        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: escrow_amount,
            labor_amount: 0,
            status: Status::Funded, // Already funded
            deadline,
            materials_released: false,
        };

        // Store the escrow
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        // Try to deposit (should panic due to status check, but first need auth)
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &client_address,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (engagement_id, token_address.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        _client.deposit(&engagement_id, &token_address);
    }

    // New tests for deadline and reclaim behavior

    #[test]
    #[should_panic(expected = "Deadline has passed; cannot deposit into this escrow")]
    fn test_deposit_fails_after_deadline() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        // Setup addresses and token
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

        // Mint some funds so transfer would succeed if not for deadline
        token_contract_client.mint(&client_address, &1000);

        // Create escrow with past deadline (ensure ledger time > 0 to avoid underflow)
        env.ledger().set_timestamp(1);
        let engagement_id = 1;
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: 500,
            labor_amount: 0,
            status: Status::Pending,
            deadline: env.ledger().timestamp().saturating_sub(1),
            materials_released: false,
        };
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        // Attempt deposit - should panic due to expired deadline
        client.deposit(&engagement_id, &token_address);
    }

    #[test]
    #[should_panic(expected = "Deadline has passed; cannot release funds")]
    fn test_release_after_deadline_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        // Setup addresses and token
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

        // Mint and fund contract
        let amount: i128 = 1000;
        let engagement_id = 1;
        env.ledger().set_timestamp(1);
        let deadline = env.ledger().timestamp().saturating_sub(1);

        token_contract_client.mint(&client_address, &amount);
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address.clone(),
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Funded,
            deadline,
            materials_released: false,
        };
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });
        token_client.transfer(&client_address, &contract_id, &amount);

        // Releasing after deadline should panic
        client.release(&engagement_id, &token_address);
    }

    #[test]
    #[should_panic(expected = "Deadline has not passed; cannot reclaim yet")]
    fn test_reclaim_fails_before_deadline() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        // Setup funded escrow with future deadline
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();

        let engagement_id = 1;
        let amount: i128 = 500;
        let deadline = env.ledger().timestamp() + 1000;
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Funded,
            deadline,
            materials_released: false,
        };
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });
        // ensure contract holds some funds so reclaim would succeed if deadline passed
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);
        token_contract_client.mint(&client_address, &amount);
        token_client.transfer(&client_address, &contract_id, &amount);

        client.reclaim(&engagement_id, &token_address);
    }

    #[test]
    fn test_client_reclaim_succeeds_after_deadline() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        // Setup funded escrow with past deadline + grace period
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

        // Set deadline in the past and timestamp beyond deadline + GRACE_PERIOD (86400s)
        let deadline = 100;
        env.ledger().set_timestamp(deadline + 86400 + 1);
        let engagement_id = 1;
        let amount: i128 = 500;
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Funded,
            deadline,
            materials_released: false,
        };
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });
        token_contract_client.mint(&client_address, &amount);
        token_client.transfer(&client_address, &contract_id, &amount);

        let before_balance = token_client.balance(&client_address);
        client.reclaim(&engagement_id, &token_address);
        let after_balance = token_client.balance(&client_address);
        assert_eq!(after_balance, before_balance + amount);

        // Event should have been published by the escrow contract (there may be additional
        // token-contract events in the list, so filter by origin address).
        let raw_events: soroban_sdk::Vec<(
            Address,
            soroban_sdk::Vec<soroban_sdk::Val>,
            soroban_sdk::Val,
        )> = env.events().all();
        let mut found = false;
        for (addr, _, _) in raw_events.into_iter() {
            if addr == contract_id {
                found = true;
                break;
            }
        }
        assert!(found, "expected at least one escrow contract event");

        let updated: Escrow = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::Escrow(engagement_id))
                .unwrap()
        });
        assert_eq!(updated.status, Status::Refunded);
    }

    #[test]
    #[should_panic]
    fn test_only_client_can_reclaim() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let client_address = Address::generate(&env);
        let other_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();

        // set up funded, expired escrow past grace period
        let engagement_id = 1;
        let amount: i128 = 100;
        let deadline = 100;
        env.ledger().set_timestamp(deadline + 86400 + 1);
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Funded,
            deadline,
            materials_released: false,
        };
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);
        token_contract_client.mint(&client_address, &amount);
        token_client.transfer(&client_address, &contract_id, &amount);

        // Attempt to reclaim as non-client
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &other_address,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &contract_id,
                fn_name: "reclaim",
                args: (engagement_id, token_address.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.reclaim(&engagement_id, &token_address);
    }

    #[test]
    #[should_panic(expected = "Escrow must be Funded to reclaim")]
    fn test_reclaim_fails_if_not_funded() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();

        let engagement_id = 1;
        let amount: i128 = 100;
        env.ledger().set_timestamp(1);
        let deadline = env.ledger().timestamp().saturating_sub(1);
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Pending,
            deadline,
            materials_released: false,
        };
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        client.reclaim(&engagement_id, &token_address);
    }

    #[test]
    #[should_panic(expected = "Escrow must be Funded to reclaim")]
    fn test_reclaim_fails_if_already_released() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

        let amount: i128 = 1000;
        let engagement_id = 1;
        // use a future deadline so we can successfully call release()
        let deadline = env.ledger().timestamp() + 1000;

        token_contract_client.mint(&client_address, &amount);

        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Funded,
            deadline,
            materials_released: false,
        };

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        token_client.transfer(&client_address, &contract_id, &amount);

        // release back to artist first
        client.release(&engagement_id, &token_address);
        // attempt reclaim after it's been released
        client.reclaim(&engagement_id, &token_address);
    }

    #[test]
    fn test_release_funds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        // Create test addresses
        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);

        // Create a mock token contract
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

        let amount: i128 = 1000;
        let engagement_id = 1;
        let deadline = env.ledger().timestamp() + 86400;

        // Mint tokens to client
        token_contract_client.mint(&client_address, &amount);

        // Create escrow
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address.clone(),
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Funded,
            deadline,
            materials_released: false,
        };

        // Store the escrow
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        // Transfer tokens to contract (simulating funded escrow)
        token_client.transfer(&client_address, &contract_id, &amount);

        assert_eq!(token_client.balance(&artisan_address), 0);
        assert_eq!(token_client.balance(&contract_id), amount);

        client.release(&engagement_id, &token_address);

        assert_eq!(token_client.balance(&artisan_address), amount);
        assert_eq!(token_client.balance(&contract_id), 0);
    }

    #[test]
    #[should_panic(expected = "Escrow is not funded")]
    fn test_release_unfunded_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();

        let amount: i128 = 1000;
        let engagement_id = 1;
        let deadline = env.ledger().timestamp() + 86400;

        // Create escrow in Pending status
        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Pending,
            deadline,
            materials_released: false,
        };

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        client.release(&engagement_id, &token_address);
    }

    #[test]
    #[should_panic(expected = "Escrow is not funded")]
    fn test_release_already_released_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client = EscrowContractClient::new(&env, &contract_id);

        let client_address = Address::generate(&env);
        let artisan_address = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();
        let token_client = soroban_sdk::token::Client::new(&env, &token_address);
        let token_contract_client =
            soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

        let amount: i128 = 1000;
        let engagement_id = 1;
        let deadline = env.ledger().timestamp() + 86400;

        token_contract_client.mint(&client_address, &amount);

        let escrow = Escrow {
            client: client_address.clone(),
            artisan: artisan_address,
            arbitrator: Address::generate(&env),
            token: token_address.clone(),
            material_amount: amount,
            labor_amount: 0,
            status: Status::Funded,
            deadline,
            materials_released: false,
        };

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(engagement_id), &escrow);
        });

        token_client.transfer(&client_address, &contract_id, &amount);

        client.release(&engagement_id, &token_address);
        client.release(&engagement_id, &token_address);
    }
}
