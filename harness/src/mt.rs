// Re-export result module from mollusk-svm-result crate
#[cfg(any(feature = "fuzz", feature = "fuzz-fd"))]
use crate::fuzz;
pub use mollusk_svm_result as result;
#[cfg(any(feature = "fuzz", feature = "fuzz-fd"))]
use mollusk_svm_result::Compare;
#[cfg(feature = "invocation-inspect-callback")]
use solana_transaction_context::InstructionAccount;
use {
    super::{MolluskInvokeContextCallback, DEFAULT_LOADER_KEY},
    crate::{
        account_store::AccountStore, compile_accounts::CompiledAccounts, epoch_stake::EpochStake,
        file, program_mt::MtProgramCache, sysvar::Sysvars,
    },
    agave_feature_set::FeatureSet,
    mollusk_svm_error::error::{MolluskError, MolluskPanic},
    mollusk_svm_result::{Check, CheckContext, Config, ContextResult, InstructionResult},
    solana_account::Account,
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_hash::Hash,
    solana_instruction::{AccountMeta, Instruction},
    solana_log_collector::LogCollector,
    solana_precompile_error::PrecompileError,
    solana_program_runtime::invoke_context::{EnvironmentConfig, InvokeContext},
    solana_pubkey::Pubkey,
    solana_svm_callback::InvokeContextCallback,
    solana_sysvar::Sysvar,
    solana_sysvar_id::SysvarId,
    solana_timings::ExecuteTimings,
    solana_transaction_context::TransactionContext,
    std::{cell::RefCell, collections::HashSet, iter::once, rc::Rc, sync::Arc, sync::RwLock},
};
pub struct MolluskMt {
    pub config: Config,
    pub compute_budget: ComputeBudget,
    pub epoch_stake: EpochStake,
    pub feature_set: FeatureSet,
    //pub logger: Option<Rc<RefCell<LogCollector>>>,
    pub program_cache: MtProgramCache,
    pub sysvars: Sysvars,

    /// The callback which can be used to inspect invoke_context
    /// and extract low-level information such as bpf traces, transaction
    /// context, detailed timings, etc.
    #[cfg(feature = "invocation-inspect-callback")]
    pub invocation_inspect_callback: Box<dyn InvocationInspectCallback>,

    /// This field stores the slot only to be able to convert to and from FD
    /// fixtures and a Mollusk instance, since FD fixtures have a
    /// "slot context". However, this field is functionally irrelevant for
    /// instruction execution, since all slot-based information for on-chain
    /// programs comes from the sysvars.
    #[cfg(feature = "fuzz-fd")]
    pub slot: u64,
}

impl Default for MolluskMt {
    fn default() -> Self {
        #[rustfmt::skip]
        solana_logger::setup_with_default(
            "solana_rbpf::vm=debug,\
             solana_runtime::message_processor=debug,\
             solana_runtime::system_instruction_processor=trace",
        );
        let compute_budget = ComputeBudget::default();
        #[cfg(feature = "fuzz")]
        let feature_set = {
            // Omit "test features" (they have the same u64 ID).
            let mut fs = FeatureSet::all_enabled();
            fs.active_mut()
                .remove(&agave_feature_set::disable_sbpf_v0_execution::id());
            fs.active_mut()
                .remove(&agave_feature_set::reenable_sbpf_v0_execution::id());
            fs
        };
        #[cfg(not(feature = "fuzz"))]
        let feature_set = FeatureSet::all_enabled();
        let program_cache = MtProgramCache::new(&feature_set, &compute_budget);
        Self {
            config: Config::default(),
            compute_budget,
            epoch_stake: EpochStake::default(),
            feature_set,
            //logger: None,
            program_cache,
            sysvars: Sysvars::default(),

            #[cfg(feature = "invocation-inspect-callback")]
            invocation_inspect_callback: Box::new(EmptyInvocationInspectCallback {}),

            #[cfg(feature = "fuzz-fd")]
            slot: 0,
        }
    }
}

impl MolluskMt {
    /// Create a new Mollusk instance containing the provided program.
    ///
    /// Attempts the load the program's ELF file from the default search paths.
    /// Once loaded, adds the program to the program cache and returns the
    /// newly created Mollusk instance.
    ///
    /// # Default Search Paths
    ///
    /// The following locations are checked in order:
    ///
    /// - `tests/fixtures`
    /// - The directory specified by the `BPF_OUT_DIR` environment variable
    /// - The directory specified by the `SBF_OUT_DIR` environment variable
    /// - The current working directory
    pub fn new(program_id: &Pubkey, program_name: &str) -> Self {
        let mut mollusk = Self::default();
        mollusk.add_program(program_id, program_name, &DEFAULT_LOADER_KEY);
        mollusk
    }

    /// Add a program to the test environment.
    ///
    /// If you intend to CPI to a program, this is likely what you want to use.
    pub fn add_program(&mut self, program_id: &Pubkey, program_name: &str, loader_key: &Pubkey) {
        let elf = file::load_program_elf(program_name);
        self.add_program_with_elf_and_loader(program_id, &elf, loader_key);
    }

    /// Add a program to the test environment using a provided ELF under a
    /// specific loader.
    ///
    /// If you intend to CPI to a program, this is likely what you want to use.
    pub fn add_program_with_elf_and_loader(
        &mut self,
        program_id: &Pubkey,
        elf: &[u8],
        loader_key: &Pubkey,
    ) {
        self.program_cache.add_program(program_id, loader_key, elf);
    }

    /// Warp the test environment to a slot by updating sysvars.
    pub fn warp_to_slot(&mut self, slot: u64) {
        self.sysvars.warp_to_slot(slot)
    }

    /// Get a sysvar from the test environment.
    pub fn get_sysvar<T>(&self) -> T
    where
        T: Sysvar + SysvarId,
    {
        // 创建一个临时的sysvar account，然后从中反序列化
        let (_, account) = if T::id() == solana_clock::Clock::id() {
            self.sysvars.keyed_account_for_clock_sysvar()
        } else if T::id() == solana_epoch_rewards::EpochRewards::id() {
            self.sysvars.keyed_account_for_epoch_rewards_sysvar()
        } else if T::id() == solana_epoch_schedule::EpochSchedule::id() {
            self.sysvars.keyed_account_for_epoch_schedule_sysvar()
        } else if T::id() == solana_sysvar::last_restart_slot::LastRestartSlot::id() {
            self.sysvars.keyed_account_for_last_restart_slot_sysvar()
        } else if T::id() == solana_rent::Rent::id() {
            self.sysvars.keyed_account_for_rent_sysvar()
        } else if T::id() == solana_slot_hashes::SlotHashes::id() {
            self.sysvars.keyed_account_for_slot_hashes_sysvar()
        } else if T::id() == solana_stake_interface::stake_history::StakeHistory::id() {
            self.sysvars.keyed_account_for_stake_history_sysvar()
        } else {
            panic!("Unsupported sysvar type: {}", T::id());
        };

        bincode::deserialize(&account.data).unwrap()
    }

    /// Set a sysvar in the test environment.
    pub fn set_sysvar<T>(&mut self, sysvar: &T)
    where
        T: Sysvar + SysvarId + Clone,
    {
        if T::id() == solana_clock::Clock::id() {
            let clock = unsafe { &*(sysvar as *const T as *const solana_clock::Clock) };
            self.sysvars.clock = clock.clone();
        } else if T::id() == solana_epoch_rewards::EpochRewards::id() {
            let epoch_rewards =
                unsafe { &*(sysvar as *const T as *const solana_epoch_rewards::EpochRewards) };
            self.sysvars.epoch_rewards = epoch_rewards.clone();
        } else if T::id() == solana_epoch_schedule::EpochSchedule::id() {
            let epoch_schedule =
                unsafe { &*(sysvar as *const T as *const solana_epoch_schedule::EpochSchedule) };
            self.sysvars.epoch_schedule = epoch_schedule.clone();
        } else if T::id() == solana_sysvar::last_restart_slot::LastRestartSlot::id() {
            let last_restart_slot = unsafe {
                &*(sysvar as *const T as *const solana_sysvar::last_restart_slot::LastRestartSlot)
            };
            self.sysvars.last_restart_slot = last_restart_slot.clone();
        } else if T::id() == solana_rent::Rent::id() {
            let rent = unsafe { &*(sysvar as *const T as *const solana_rent::Rent) };
            self.sysvars.rent = rent.clone();
        } else if T::id() == solana_slot_hashes::SlotHashes::id() {
            let slot_hashes =
                unsafe { &*(sysvar as *const T as *const solana_slot_hashes::SlotHashes) };
            // SlotHashes doesn't implement Clone, so we need to reconstruct it
            let slot_hash_entries: Vec<(u64, Hash)> = slot_hashes.as_slice().to_vec();
            self.sysvars.slot_hashes = solana_slot_hashes::SlotHashes::new(&slot_hash_entries);
        } else if T::id() == solana_stake_interface::stake_history::StakeHistory::id() {
            let stake_history = unsafe {
                &*(sysvar as *const T as *const solana_stake_interface::stake_history::StakeHistory)
            };
            self.sysvars.stake_history = stake_history.clone();
        } else {
            panic!("Unsupported sysvar type: {}", T::id());
        }
    }

    /// Expire the current blockhash by creating a new one.
    pub fn expire_blockhash(&mut self) {
        // Create a new blockhash based on the current slot + timestamp
        let current_slot = self.sysvars.clock.slot;
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut hash_data = [0u8; 32];
        hash_data[0..8].copy_from_slice(&current_slot.to_le_bytes());
        hash_data[8..16].copy_from_slice(&current_time.to_le_bytes());
        hash_data[16] = 0xFF; // Add some entropy

        let new_hash = Hash::new_from_array(hash_data);

        // To truly expire the blockhash, we need to add a new slot hash entry
        // Add the new hash for the next slot to simulate blockhash progression
        let next_slot = current_slot + 1;
        self.sysvars.slot_hashes.add(next_slot, new_hash);

        // Also update the clock to reflect the progression
        self.sysvars.clock.slot = next_slot;
    }
    /// Returns minimum balance required to make an account with specified data length rent exempt.
    pub fn minimum_balance_for_rent_exemption(&self, data_len: usize) -> u64 {
        1.max(
            self.sysvars
                .rent
                .minimum_balance(data_len),
        )
    }

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment. Simply returns the result.
    pub fn process_instruction(
        &self,
        instruction: &Instruction,
        accounts: &[(Pubkey, Account)],
    ) -> InstructionResult {
        let mut compute_units_consumed = 0;
        let mut timings = ExecuteTimings::default();

        let loader_key = if crate::program::precompile_keys::is_precompile(&instruction.program_id)
        {
            crate::program::loader_keys::NATIVE_LOADER
        } else {
            self.program_cache
                .load_program(&instruction.program_id)
                .or_panic_with(MolluskError::ProgramNotCached(&instruction.program_id))
                .account_owner()
        };

        let CompiledAccounts {
            program_id_index,
            instruction_accounts,
            transaction_accounts,
        } = crate::compile_accounts::compile_accounts(instruction, accounts, loader_key);

        let mut transaction_context = TransactionContext::new(
            transaction_accounts,
            self.sysvars.rent.clone(),
            self.compute_budget.max_instruction_stack_depth,
            self.compute_budget.max_instruction_trace_length,
        );

        let invoke_result = {
            let mut program_cache = self.program_cache.cache();
            let callback = MolluskInvokeContextCallback {
                epoch_stake: &self.epoch_stake,
                feature_set: &self.feature_set,
            };
            let runtime_features = self.feature_set.runtime_features();
            let sysvar_cache = self.sysvars.setup_sysvar_cache(accounts);
            let mut invoke_context = InvokeContext::new(
                &mut transaction_context,
                &mut program_cache,
                EnvironmentConfig::new(
                    Hash::default(),
                    /* blockhash_lamports_per_signature */ 5000, // The default value
                    &callback,
                    &runtime_features,
                    &sysvar_cache,
                ),
                //self.logger.clone(),
                None,
                self.compute_budget.to_budget(),
                self.compute_budget.to_cost(),
            );

            #[cfg(feature = "invocation-inspect-callback")]
            self.invocation_inspect_callback.before_invocation(
                &instruction.program_id,
                &instruction.data,
                &instruction_accounts,
                &invoke_context,
            );

            let result = if invoke_context.is_precompile(&instruction.program_id) {
                invoke_context.process_precompile(
                    &instruction.program_id,
                    &instruction.data,
                    &instruction_accounts,
                    &[program_id_index],
                    std::iter::once(instruction.data.as_ref()),
                )
            } else {
                invoke_context.process_instruction(
                    &instruction.data,
                    &instruction_accounts,
                    &[program_id_index],
                    &mut compute_units_consumed,
                    &mut timings,
                )
            };

            #[cfg(feature = "invocation-inspect-callback")]
            self.invocation_inspect_callback
                .after_invocation(&invoke_context);

            result
        };

        let return_data = transaction_context.get_return_data().1.to_vec();

        let resulting_accounts: Vec<(Pubkey, Account)> = if invoke_result.is_ok() {
            accounts
                .iter()
                .map(|(pubkey, account)| {
                    transaction_context
                        .find_index_of_account(pubkey)
                        .map(|index| {
                            let resulting_account = transaction_context
                                .get_account_at_index(index)
                                .unwrap()
                                .borrow()
                                .clone()
                                .into();
                            (*pubkey, resulting_account)
                        })
                        .unwrap_or((*pubkey, account.clone()))
                })
                .collect()
        } else {
            accounts.to_vec()
        };

        InstructionResult {
            compute_units_consumed,
            execution_time: timings.details.execute_us.0,
            program_result: invoke_result.clone().into(),
            raw_result: invoke_result,
            return_data,
            resulting_accounts,
        }
    }

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment. Simply returns the result.
    pub fn process_instruction_log(
        &self,
        instruction: &Instruction,
        accounts: &[(Pubkey, Account)],
        log: Option<Rc<RefCell<LogCollector>>>,
    ) -> (InstructionResult, TransactionContext) {
        let mut compute_units_consumed = 0;
        let mut timings = ExecuteTimings::default();

        let loader_key = if crate::program::precompile_keys::is_precompile(&instruction.program_id)
        {
            crate::program::loader_keys::NATIVE_LOADER
        } else {
            self.program_cache
                .load_program(&instruction.program_id)
                .or_panic_with(MolluskError::ProgramNotCached(&instruction.program_id))
                .account_owner()
        };

        let CompiledAccounts {
            program_id_index,
            instruction_accounts,
            transaction_accounts,
        } = crate::compile_accounts::compile_accounts(instruction, accounts, loader_key);

        let mut transaction_context = TransactionContext::new(
            transaction_accounts,
            self.sysvars.rent.clone(),
            self.compute_budget.max_instruction_stack_depth,
            self.compute_budget.max_instruction_trace_length,
        );

        let invoke_result = {
            let mut program_cache = self.program_cache.cache();
            let callback = MolluskInvokeContextCallback {
                epoch_stake: &self.epoch_stake,
                feature_set: &self.feature_set,
            };
            let runtime_features = self.feature_set.runtime_features();
            let sysvar_cache = self.sysvars.setup_sysvar_cache(accounts);
            let mut invoke_context = InvokeContext::new(
                &mut transaction_context,
                &mut program_cache,
                EnvironmentConfig::new(
                    Hash::default(),
                    /* blockhash_lamports_per_signature */ 5000, // The default value
                    &callback,
                    &runtime_features,
                    &sysvar_cache,
                ),
                log,
                self.compute_budget.to_budget(),
                self.compute_budget.to_cost(),
            );

            #[cfg(feature = "invocation-inspect-callback")]
            self.invocation_inspect_callback.before_invocation(
                &instruction.program_id,
                &instruction.data,
                &instruction_accounts,
                &invoke_context,
            );

            let result = if invoke_context.is_precompile(&instruction.program_id) {
                invoke_context.process_precompile(
                    &instruction.program_id,
                    &instruction.data,
                    &instruction_accounts,
                    &[program_id_index],
                    std::iter::once(instruction.data.as_ref()),
                )
            } else {
                invoke_context.process_instruction(
                    &instruction.data,
                    &instruction_accounts,
                    &[program_id_index],
                    &mut compute_units_consumed,
                    &mut timings,
                )
            };

            #[cfg(feature = "invocation-inspect-callback")]
            self.invocation_inspect_callback
                .after_invocation(&invoke_context);

            result
        };

        let return_data = transaction_context.get_return_data().1.to_vec();

        let resulting_accounts: Vec<(Pubkey, Account)> = if invoke_result.is_ok() {
            accounts
                .iter()
                .map(|(pubkey, account)| {
                    transaction_context
                        .find_index_of_account(pubkey)
                        .map(|index| {
                            let resulting_account = transaction_context
                                .get_account_at_index(index)
                                .unwrap()
                                .borrow()
                                .clone()
                                .into();
                            (*pubkey, resulting_account)
                        })
                        .unwrap_or((*pubkey, account.clone()))
                })
                .collect()
        } else {
            accounts.to_vec()
        };

        (
            InstructionResult {
                compute_units_consumed,
                execution_time: timings.details.execute_us.0,
                program_result: invoke_result.clone().into(),
                raw_result: invoke_result,
                return_data,
                resulting_accounts,
            },
            transaction_context,
        )
    }

    /// Process a chain of instructions using the minified Solana Virtual
    /// Machine (SVM) environment. The returned result is an
    /// `InstructionResult`, containing:
    ///
    /// * `compute_units_consumed`: The total compute units consumed across all
    ///   instructions.
    /// * `execution_time`: The total execution time across all instructions.
    /// * `program_result`: The program result of the _last_ instruction.
    /// * `resulting_accounts`: The resulting accounts after the _last_
    ///   instruction.
    pub fn process_instruction_chain(
        &self,
        instructions: &[Instruction],
        accounts: &[(Pubkey, Account)],
    ) -> InstructionResult {
        let mut result = InstructionResult {
            resulting_accounts: accounts.to_vec(),
            ..Default::default()
        };

        for instruction in instructions {
            let this_result = self.process_instruction(instruction, &result.resulting_accounts);

            result.absorb(this_result);

            if result.program_result.is_err() {
                break;
            }
        }

        result
    }

    /// Process a chain of instructions using the minified Solana Virtual
    /// Machine (SVM) environment. The returned result is an
    /// `InstructionResult`, containing:
    ///
    /// * `compute_units_consumed`: The total compute units consumed across all
    ///   instructions.
    /// * `execution_time`: The total execution time across all instructions.
    /// * `program_result`: The program result of the _last_ instruction.
    /// * `resulting_accounts`: The resulting accounts after the _last_
    ///   instruction.
    pub fn process_instruction_chain_log(
        &self,
        instructions: &[Instruction],
        accounts: &[(Pubkey, Account)],
        log: Option<Rc<RefCell<LogCollector>>>,
    ) -> (InstructionResult, TransactionContext) {
        let mut result = InstructionResult {
            resulting_accounts: accounts.to_vec(),
            ..Default::default()
        };
        // Initialize tc with a default TransactionContext in case the loop doesn't execute
        let mut tc = TransactionContext::new(vec![], solana_rent::Rent::default(), 0, 0);

        for instruction in instructions {
            let this_result =
                self.process_instruction_log(instruction, &result.resulting_accounts, log.clone());

            result.absorb(this_result.0);
            tc = this_result.1;

            if result.program_result.is_err() {
                break;
            }
        }

        (result, tc)
    }

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment, then perform checks on the result. Panics if any checks
    /// fail.
    ///
    /// For `fuzz` feature only:
    ///
    /// If the `EJECT_FUZZ_FIXTURES` environment variable is set, this function
    /// will convert the provided test to a fuzz fixture and write it to the
    /// provided directory.
    ///
    /// ```ignore
    /// EJECT_FUZZ_FIXTURES="./fuzz-fixtures" cargo test-sbf ...
    /// ```
    ///
    /// You can also provide `EJECT_FUZZ_FIXTURES_JSON` to write the fixture in
    /// JSON format.
    ///
    /// The `fuzz-fd` feature works the same way, but the variables require
    /// the `_FD` suffix, in case both features are active together
    /// (ie. `EJECT_FUZZ_FIXTURES_FD`). This will generate Firedancer fuzzing
    /// fixtures, which are structured a bit differently than Mollusk's own
    /// protobuf layouts.
    pub fn process_and_validate_instruction(
        &self,
        instruction: &Instruction,
        accounts: &[(Pubkey, Account)],
        checks: &[Check],
    ) -> InstructionResult {
        let result = self.process_instruction(instruction, accounts);

        /*#[cfg(any(feature = "fuzz", feature = "fuzz-fd"))]
        fuzz::generate_fixtures_from_mollusk_test(self, instruction, accounts, &result);

        result.run_checks(checks, &self.config, self);*/
        result
    }

    /// Process a chain of instructions using the minified Solana Virtual
    /// Machine (SVM) environment, then perform checks on the result.
    /// Panics if any checks fail.
    ///
    /// For `fuzz` feature only:
    ///
    /// Similar to `process_and_validate_instruction`, if the
    /// `EJECT_FUZZ_FIXTURES` environment variable is set, this function will
    /// convert the provided test to a set of fuzz fixtures - each of which
    /// corresponds to a single instruction in the chain - and write them to
    /// the provided directory.
    ///
    /// ```ignore
    /// EJECT_FUZZ_FIXTURES="./fuzz-fixtures" cargo test-sbf ...
    /// ```
    ///
    /// You can also provide `EJECT_FUZZ_FIXTURES_JSON` to write the fixture in
    /// JSON format.
    ///
    /// The `fuzz-fd` feature works the same way, but the variables require
    /// the `_FD` suffix, in case both features are active together
    /// (ie. `EJECT_FUZZ_FIXTURES_FD`). This will generate Firedancer fuzzing
    /// fixtures, which are structured a bit differently than Mollusk's own
    /// protobuf layouts.
    pub fn process_and_validate_instruction_chain(
        &self,
        instructions: &[(&Instruction, &[Check])],
        accounts: &[(Pubkey, Account)],
    ) -> InstructionResult {
        let mut result = InstructionResult {
            resulting_accounts: accounts.to_vec(),
            ..Default::default()
        };

        for (instruction, checks) in instructions.iter() {
            let this_result = self.process_and_validate_instruction(
                instruction,
                &result.resulting_accounts,
                checks,
            );

            result.absorb(this_result);

            if result.program_result.is_err() {
                break;
            }
        }

        result
    }

    #[cfg(feature = "fuzz")]
    /// Process a fuzz fixture using the minified Solana Virtual Machine (SVM)
    /// environment.
    ///
    /// Fixtures provide an API to `decode` a raw blob, as well as read
    /// fixtures from files. Those fixtures can then be provided to this
    /// function to process them and get a Mollusk result.
    ///
    /// Note: This is a mutable method on `Mollusk`, since loading a fixture
    /// into the test environment will alter `Mollusk` values, such as compute
    /// budget and sysvars. However, the program cache remains unchanged.
    ///
    /// Therefore, developers can provision a `Mollusk` instance, set up their
    /// desired program cache, and then run a series of fixtures against that
    /// `Mollusk` instance (and cache).
    pub fn process_fixture(
        &mut self,
        fixture: &mollusk_svm_fuzz_fixture::Fixture,
    ) -> InstructionResult {
        let fuzz::mollusk::ParsedFixtureContext {
            accounts,
            compute_budget,
            feature_set,
            instruction,
            sysvars,
        } = fuzz::mollusk::parse_fixture_context(&fixture.input);
        self.compute_budget = compute_budget;
        self.feature_set = feature_set;
        self.sysvars = sysvars;
        self.process_instruction(&instruction, &accounts)
    }

    #[cfg(feature = "fuzz")]
    /// Process a fuzz fixture using the minified Solana Virtual Machine (SVM)
    /// environment and compare the result against the fixture's effects.
    ///
    /// Fixtures provide an API to `decode` a raw blob, as well as read
    /// fixtures from files. Those fixtures can then be provided to this
    /// function to process them and get a Mollusk result.
    ///
    ///
    /// Note: This is a mutable method on `Mollusk`, since loading a fixture
    /// into the test environment will alter `Mollusk` values, such as compute
    /// budget and sysvars. However, the program cache remains unchanged.
    ///
    /// Therefore, developers can provision a `Mollusk` instance, set up their
    /// desired program cache, and then run a series of fixtures against that
    /// `Mollusk` instance (and cache).
    ///
    /// Note: To compare the result against the entire fixture effects, pass
    /// `&[FixtureCheck::All]` for `checks`.
    pub fn process_and_validate_fixture(
        &mut self,
        fixture: &mollusk_svm_fuzz_fixture::Fixture,
    ) -> InstructionResult {
        let result = self.process_fixture(fixture);
        InstructionResult::from(&fixture.output).compare_with_config(
            &result,
            &Compare::everything(),
            &self.config,
        );
        result
    }

    #[cfg(feature = "fuzz")]
    /// a specific set of checks.
    ///
    /// This is useful for when you may not want to compare the entire effects,
    /// such as omitting comparisons of compute units consumed.
    /// Process a fuzz fixture using the minified Solana Virtual Machine (SVM)
    /// environment and compare the result against the fixture's effects using
    /// a specific set of checks.
    ///
    /// This is useful for when you may not want to compare the entire effects,
    /// such as omitting comparisons of compute units consumed.
    ///
    /// Fixtures provide an API to `decode` a raw blob, as well as read
    /// fixtures from files. Those fixtures can then be provided to this
    /// function to process them and get a Mollusk result.
    ///
    ///
    /// Note: This is a mutable method on `Mollusk`, since loading a fixture
    /// into the test environment will alter `Mollusk` values, such as compute
    /// budget and sysvars. However, the program cache remains unchanged.
    ///
    /// Therefore, developers can provision a `Mollusk` instance, set up their
    /// desired program cache, and then run a series of fixtures against that
    /// `Mollusk` instance (and cache).
    ///
    /// Note: To compare the result against the entire fixture effects, pass
    /// `&[FixtureCheck::All]` for `checks`.
    pub fn process_and_partially_validate_fixture(
        &mut self,
        fixture: &mollusk_svm_fuzz_fixture::Fixture,
        checks: &[Compare],
    ) -> InstructionResult {
        let result = self.process_fixture(fixture);
        let expected = InstructionResult::from(&fixture.output);
        result.compare_with_config(&expected, checks, &self.config);
        result
    }

    #[cfg(feature = "fuzz-fd")]
    /// Process a Firedancer fuzz fixture using the minified Solana Virtual
    /// Machine (SVM) environment.
    ///
    /// Fixtures provide an API to `decode` a raw blob, as well as read
    /// fixtures from files. Those fixtures can then be provided to this
    /// function to process them and get a Mollusk result.
    ///
    /// Note: This is a mutable method on `Mollusk`, since loading a fixture
    /// into the test environment will alter `Mollusk` values, such as compute
    /// budget and sysvars. However, the program cache remains unchanged.
    ///
    /// Therefore, developers can provision a `Mollusk` instance, set up their
    /// desired program cache, and then run a series of fixtures against that
    /// `Mollusk` instance (and cache).
    pub fn process_firedancer_fixture(
        &mut self,
        fixture: &mollusk_svm_fuzz_fixture_firedancer::Fixture,
    ) -> InstructionResult {
        let fuzz::firedancer::ParsedFixtureContext {
            accounts,
            compute_budget,
            feature_set,
            instruction,
            slot,
        } = fuzz::firedancer::parse_fixture_context(&fixture.input);
        self.compute_budget = compute_budget;
        self.feature_set = feature_set;
        self.slot = slot;
        self.process_instruction(&instruction, &accounts)
    }

    #[cfg(feature = "fuzz-fd")]
    /// Process a Firedancer fuzz fixture using the minified Solana Virtual
    /// Machine (SVM) environment and compare the result against the
    /// fixture's effects.
    ///
    /// Fixtures provide an API to `decode` a raw blob, as well as read
    /// fixtures from files. Those fixtures can then be provided to this
    /// function to process them and get a Mollusk result.
    ///
    ///
    /// Note: This is a mutable method on `Mollusk`, since loading a fixture
    /// into the test environment will alter `Mollusk` values, such as compute
    /// budget and sysvars. However, the program cache remains unchanged.
    ///
    /// Therefore, developers can provision a `Mollusk` instance, set up their
    /// desired program cache, and then run a series of fixtures against that
    /// `Mollusk` instance (and cache).
    ///
    /// Note: To compare the result against the entire fixture effects, pass
    /// `&[FixtureCheck::All]` for `checks`.
    pub fn process_and_validate_firedancer_fixture(
        &mut self,
        fixture: &mollusk_svm_fuzz_fixture_firedancer::Fixture,
    ) -> InstructionResult {
        let fuzz::firedancer::ParsedFixtureContext {
            accounts,
            compute_budget,
            feature_set,
            instruction,
            slot,
        } = fuzz::firedancer::parse_fixture_context(&fixture.input);
        self.compute_budget = compute_budget;
        self.feature_set = feature_set;
        self.slot = slot;

        let result = self.process_instruction(&instruction, &accounts);
        let expected_result = fuzz::firedancer::parse_fixture_effects(
            &accounts,
            self.compute_budget.compute_unit_limit,
            &fixture.output,
        );

        expected_result.compare_with_config(&result, &Compare::everything(), &self.config);
        result
    }
    /*
        #[cfg(feature = "fuzz-fd")]
        /// Process a Firedancer fuzz fixture using the minified Solana Virtual
        /// Machine (SVM) environment and compare the result against the
        /// fixture's effects using a specific set of checks.
        ///
        /// This is useful for when you may not want to compare the entire effects,
        /// such as omitting comparisons of compute units consumed.
        ///
        /// Fixtures provide an API to `decode` a raw blob, as well as read
        /// fixtures from files. Those fixtures can then be provided to this
        /// function to process them and get a Mollusk result.
        ///
        ///
        /// Note: This is a mutable method on `Mollusk`, since loading a fixture
        /// into the test environment will alter `Mollusk` values, such as compute
        /// budget and sysvars. However, the program cache remains unchanged.
        ///
        /// Therefore, developers can provision a `Mollusk` instance, set up their
        /// desired program cache, and then run a series of fixtures against that
        /// `Mollusk` instance (and cache).
        ///
        /// Note: To compare the result against the entire fixture effects, pass
        /// `&[FixtureCheck::All]` for `checks`.
        pub fn process_and_partially_validate_firedancer_fixture(
            &mut self,
            fixture: &mollusk_svm_fuzz_fixture_firedancer::Fixture,
            checks: &[Compare],
        ) -> InstructionResult {
            let fuzz::firedancer::ParsedFixtureContext {
                accounts,
                compute_budget,
                feature_set,
                instruction,
                slot,
            } = fuzz::firedancer::parse_fixture_context(&fixture.input);
            self.compute_budget = compute_budget;
            self.feature_set = feature_set;
            self.slot = slot;

            let result = self.process_instruction(&instruction, &accounts);
            let expected = fuzz::firedancer::parse_fixture_effects(
                &accounts,
                self.compute_budget.compute_unit_limit,
                &fixture.output,
            );

            result.compare_with_config(&expected, checks, &self.config);
            result
        }
    */
    /// Convert this `Mollusk` instance into a `MolluskContext` for stateful
    /// testing.
    ///
    /// Creates a context wrapper that manages persistent state between
    /// instruction executions, starting with the provided account store.
    ///
    /// See [`MolluskContext`] for more details on how to use it.
    pub fn with_context<AS: AccountStore>(self, mut account_store: AS) -> MolluskContextMt<AS> {
        // For convenience, load all program accounts into the account store,
        // but only if they don't exist.
        self.program_cache
            .get_all_keyed_program_accounts()
            .into_iter()
            .for_each(|(pubkey, account)| {
                if account_store.get_account(&pubkey).is_none() {
                    account_store.store_account(pubkey, account);
                }
            });
        MolluskContextMt {
            mollusk: self,
            account_store: Arc::new(RwLock::new(account_store)),
        }
    }
}
/// A stateful wrapper around `Mollusk` that provides additional context and
/// convenience features for testing programs.
///
/// `MolluskContext` maintains persistent state between instruction executions,
/// starting with an account store that automatically manages account
/// lifecycles. This makes it ideal for complex testing scenarios involving
/// multiple instructions, instruction chains, and stateful program
/// interactions.
///
/// Note: Account state is only persisted if the instruction execution
/// was successful. If an instruction fails, the account state will not
/// be updated.
///
/// The API is functionally identical to `Mollusk` but with enhanced state
/// management and a streamlined interface. Namely, the input `accounts` slice
/// is no longer required, and the returned result does not contain a
/// `resulting_accounts` field.
pub struct MolluskContextMt<AS: AccountStore> {
    pub mollusk: MolluskMt,
    pub account_store: Arc<RwLock<AS>>,
}
impl<AS: AccountStore> Clone for MolluskContextMt<AS> {
    fn clone(&self) -> Self {
        self.clone()
    }
}

impl<AS: AccountStore> MolluskContextMt<AS> {
    fn load_accounts_for_instructions<'a>(
        &self,
        instructions: impl Iterator<Item = &'a Instruction>,
    ) -> Vec<(Pubkey, Account)> {
        let mut seen = HashSet::new();
        let mut accounts = Vec::new();
        let store = self.account_store.read().unwrap();
        instructions.for_each(|instruction| {
            instruction
                .accounts
                .iter()
                .for_each(|AccountMeta { pubkey, .. }| {
                    if seen.insert(*pubkey) {
                        let account = store
                            .get_account(pubkey)
                            .unwrap_or_else(|| store.default_account(pubkey));
                        accounts.push((*pubkey, account));
                    }
                });
        });
        accounts
    }

    fn consume_mollusk_result(&self, result: InstructionResult) -> ContextResult {
        let InstructionResult {
            compute_units_consumed,
            execution_time,
            program_result,
            raw_result,
            return_data,
            resulting_accounts,
        } = result;

        let mut store = self.account_store.write().unwrap();
        for (pubkey, account) in resulting_accounts {
            store.store_account(pubkey, account);
        }

        ContextResult {
            compute_units_consumed,
            execution_time,
            program_result,
            raw_result,
            return_data,
        }
    }

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment. Simply returns the result.
    pub fn process_instruction(&self, instruction: &Instruction) -> ContextResult {
        let accounts = self.load_accounts_for_instructions(once(instruction));
        let result = self.mollusk.process_instruction(instruction, &accounts);
        self.consume_mollusk_result(result)
    }

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment. Simply returns the result.
    pub fn process_instruction_log(
        &self,
        instruction: &Instruction,
        log: Option<Rc<RefCell<LogCollector>>>,
    ) -> (ContextResult, TransactionContext) {
        let accounts = self.load_accounts_for_instructions(once(instruction));

        let result = self
            .mollusk
            .process_instruction_log(instruction, &accounts, log);
        (self.consume_mollusk_result(result.0), result.1)
    }

    /// Process a chain of instructions using the minified Solana Virtual
    /// Machine (SVM) environment.
    pub fn process_instruction_chain(&self, instructions: &[Instruction]) -> ContextResult {
        let accounts = self.load_accounts_for_instructions(instructions.iter());
        let result = self
            .mollusk
            .process_instruction_chain(instructions, &accounts);
        self.consume_mollusk_result(result)
    }

    /// Process a chain of instructions using the minified Solana Virtual
    /// Machine (SVM) environment.
    pub fn process_instruction_chain_log(
        &self,
        instructions: &[Instruction],
        log: Option<Rc<RefCell<LogCollector>>>,
    ) -> (ContextResult, TransactionContext) {
        let accounts = self.load_accounts_for_instructions(instructions.iter());
        let result = self
            .mollusk
            .process_instruction_chain_log(instructions, &accounts, log);
        (self.consume_mollusk_result(result.0), result.1)
    }

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment, then perform checks on the result.
    pub fn process_and_validate_instruction(
        &self,
        instruction: &Instruction,
        checks: &[Check],
    ) -> ContextResult {
        let accounts = self.load_accounts_for_instructions(once(instruction));
        let result = self
            .mollusk
            .process_and_validate_instruction(instruction, &accounts, checks);
        self.consume_mollusk_result(result)
    }

    /// Process a chain of instructions using the minified Solana Virtual
    /// Machine (SVM) environment, then perform checks on the result.
    pub fn process_and_validate_instruction_chain(
        &self,
        instructions: &[(&Instruction, &[Check])],
    ) -> ContextResult {
        let accounts = self.load_accounts_for_instructions(
            instructions.iter().map(|(instruction, _)| *instruction),
        );
        let result = self
            .mollusk
            .process_and_validate_instruction_chain(instructions, &accounts);
        self.consume_mollusk_result(result)
    }
}
