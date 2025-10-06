use crate::file;
use crate::DEFAULT_LOADER_KEY;
pub use mollusk_svm_result as result;
#[cfg(any(feature = "fuzz", feature = "fuzz-fd"))]
use mollusk_svm_result::Compare;
#[cfg(feature = "invocation-inspect-callback")]
use solana_transaction_context::InstructionAccount;
use {
    crate::{
        account_store::AccountStore, compile_accounts::CompiledAccounts, epoch_stake::EpochStake,
        program::ProgramCache, sysvar::Sysvars,
    },
    agave_feature_set::FeatureSet,
    itertools,
    mollusk_svm_error::error::{MolluskError, MolluskPanic},
    mollusk_svm_keys::{
        accounts::{
            compile_instruction_accounts, compile_instruction_without_data,
            compile_transaction_accounts_from_store,
        },
        keys::KeyMap,
    },
    mollusk_svm_result::{Check, CheckContext, Config, InstructionResult},
    solana_account::{state_traits::StateMut, Account, WritableAccount},
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_hash::Hash,
    solana_instruction::{AccountMeta, Instruction},
    solana_loader_v3_interface::state::UpgradeableLoaderState,
    solana_precompile_error::PrecompileError,
    solana_program_runtime::invoke_context::{EnvironmentConfig, InvokeContext},
    solana_pubkey::Pubkey,
    solana_stake_interface::stake_history::StakeHistory as SysvarStakeHistory,
    solana_svm_callback::InvokeContextCallback,
    solana_svm_log_collector::LogCollector,
    solana_svm_timings::ExecuteTimings,
    solana_system_program,
    solana_sysvar::Sysvar,
    solana_sysvar_id::SysvarId,
    solana_transaction_context::InstructionAccount,
    solana_transaction_context::TransactionContext,
    std::{
        cell::RefCell, collections::HashSet, iter::once, rc::Rc, sync::Arc, sync::RwLock,
        sync::RwLockWriteGuard,
    },
};
pub struct MolluskMt {
    pub config: Config,
    pub compute_budget: ComputeBudget,
    pub epoch_stake: EpochStake,
    pub feature_set: FeatureSet,
    //pub logger: Option<Rc<RefCell<LogCollector>>>,
    pub program_cache: ProgramCache,
    pub sysvars: Sysvars,

    /// The callback which can be used to inspect invoke_context
    /// and extract low-level information such as bpf traces, transaction
    /// context, detailed timings, etc.
    #[cfg(feature = "invocation-inspect-callback")]
    pub invocation_inspect_callback: Box<dyn crate::InvocationInspectCallback>,

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
        let compute_budget = ComputeBudget::new_with_defaults(true);

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
        let program_cache = ProgramCache::new(&feature_set, &compute_budget);
        Self {
            config: Config::default(),
            compute_budget,
            epoch_stake: EpochStake::default(),
            feature_set,
            //logger: None,
            program_cache,
            sysvars: Sysvars::default(),

            #[cfg(feature = "invocation-inspect-callback")]
            invocation_inspect_callback: Box::new(crate::EmptyInvocationInspectCallback {}),

            #[cfg(feature = "fuzz-fd")]
            slot: 0,
        }
    }
}

impl CheckContext for MolluskMt {
    fn is_rent_exempt(&self, lamports: u64, space: usize, owner: Pubkey) -> bool {
        owner.eq(&Pubkey::default()) && lamports == 0
            || self.sysvars.rent.is_exempt(lamports, space)
    }
}

struct MolluskInvokeContextCallback<'a> {
    feature_set: &'a FeatureSet,
    epoch_stake: &'a EpochStake,
}

impl InvokeContextCallback for MolluskInvokeContextCallback<'_> {
    fn get_epoch_stake(&self) -> u64 {
        self.epoch_stake.values().sum()
    }

    fn get_epoch_stake_for_vote_account(&self, vote_address: &Pubkey) -> u64 {
        self.epoch_stake.get(vote_address).copied().unwrap_or(0)
    }

    fn is_precompile(&self, program_id: &Pubkey) -> bool {
        agave_precompiles::is_precompile(program_id, |feature_id| {
            self.feature_set.is_active(feature_id)
        })
    }

    fn process_precompile(
        &self,
        program_id: &Pubkey,
        data: &[u8],
        instruction_datas: Vec<&[u8]>,
    ) -> Result<(), PrecompileError> {
        if let Some(precompile) = agave_precompiles::get_precompile(program_id, |feature_id| {
            self.feature_set.is_active(feature_id)
        }) {
            precompile.verify(data, &instruction_datas, self.feature_set)
        } else {
            Err(PrecompileError::InvalidPublicKey)
        }
    }
}

impl MolluskMt {
    /// Create a new Mollusk instance containing the provided program.
    ///
    /// Attempts to load the program's ELF file from the default search paths.
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
        T: Sysvar + SysvarId + serde::de::DeserializeOwned,
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
        } else if T::id() == SysvarStakeHistory::id() {
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
            let slot_hash_entries: Vec<(u64, solana_hash::Hash)> = slot_hashes.as_slice().to_vec();
            self.sysvars.slot_hashes = solana_slot_hashes::SlotHashes::new(&slot_hash_entries);
        } else if T::id() == SysvarStakeHistory::id() {
            let stake_history = unsafe { &*(sysvar as *const T as *const SysvarStakeHistory) };
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

        let new_hash = solana_hash::Hash::new_from_array(hash_data);

        // To truly expire the blockhash, we need to add a new slot hash entry
        // Add the new hash for the next slot to simulate blockhash progression
        let next_slot = current_slot + 1;
        self.sysvars.slot_hashes.add(next_slot, new_hash);

        // Also update the clock to reflect the progression
        self.sysvars.clock.slot = next_slot;
    }

    /// Returns minimum balance required to make an account with specified data length rent exempt.
    pub fn minimum_balance_for_rent_exemption(&self, data_len: usize) -> u64 {
        1.max(self.sysvars.rent.minimum_balance(data_len))
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

            // Configure the next instruction frame for this invocation.
            invoke_context
                .transaction_context
                .configure_next_instruction_for_tests(
                    program_id_index,
                    instruction_accounts.clone(),
                    &instruction.data,
                )
                .expect("failed to configure next instruction");

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
                    std::iter::once(instruction.data.as_ref()),
                )
            } else {
                invoke_context.process_instruction(&mut compute_units_consumed, &mut timings)
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
                                .accounts()
                                .try_borrow(index)
                                .unwrap()
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
    ) -> (
        InstructionResult,
        solana_transaction_context::TransactionContext,
    ) {
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
                //None,
                log,
                self.compute_budget.to_budget(),
                self.compute_budget.to_cost(),
            );

            // Configure the next instruction frame for this invocation.
            invoke_context
                .transaction_context
                .configure_next_instruction_for_tests(
                    program_id_index,
                    instruction_accounts.clone(),
                    &instruction.data,
                )
                .expect("failed to configure next instruction");

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
                    std::iter::once(instruction.data.as_ref()),
                )
            } else {
                invoke_context.process_instruction(&mut compute_units_consumed, &mut timings)
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
                                .accounts()
                                .try_borrow(index)
                                .unwrap()
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

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment with a provided TransactionContext.
    pub fn process_instruction_tx(
        &self,
        instruction: &Instruction,
        transaction_context: &mut TransactionContext,
        accounts: &[(Pubkey, Account)],
        log: Option<Rc<RefCell<LogCollector>>>,
    ) -> InstructionResult {
        let mut compute_units_consumed = 0;
        let mut timings = ExecuteTimings::default();

        let loader_key = if crate::program::precompile_keys::is_precompile(&instruction.program_id)
        {
            println!(
                "Using NATIVE_LOADER for precompile: {:?}",
                instruction.program_id
            );
            crate::program::loader_keys::NATIVE_LOADER
        } else if instruction.program_id == solana_system_program::id() {
            println!(
                "Using NATIVE_LOADER for system program: {:?}",
                instruction.program_id
            );
            crate::program::loader_keys::NATIVE_LOADER
        } else {
            println!("Loading program from cache: {:?}", instruction.program_id);
            self.program_cache
                .load_program(&instruction.program_id)
                .or_panic_with(MolluskError::ProgramNotCached(&instruction.program_id))
                .account_owner()
        };

        let CompiledAccounts {
            program_id_index,
            instruction_accounts,
            transaction_accounts: _,
        } = crate::compile_accounts::compile_accounts(instruction, accounts, loader_key);
        //transaction_context

        let invoke_result = {
            println!(
                "Program cache has system program: {}",
                self.program_cache
                    .load_program(&solana_system_program::id())
                    .is_some()
            );
            let mut program_cache = self.program_cache.cache();
            let callback = MolluskInvokeContextCallback {
                epoch_stake: &self.epoch_stake,
                feature_set: &self.feature_set,
            };
            let runtime_features = self.feature_set.runtime_features();
            let sysvar_cache = self.sysvars.setup_sysvar_cache(accounts);
            let mut invoke_context = InvokeContext::new(
                transaction_context,
                &mut program_cache,
                EnvironmentConfig::new(
                    Hash::default(),
                    /* blockhash_lamports_per_signature */ 5000, // The default value
                    &callback,
                    &runtime_features,
                    &sysvar_cache,
                ),
                //self.logger.clone(),
                //None,
                log,
                self.compute_budget.to_budget(),
                self.compute_budget.to_cost(),
            );

            // Configure the next instruction frame for this invocation.
            invoke_context
                .transaction_context
                .configure_next_instruction_for_tests(
                    program_id_index,
                    instruction_accounts.clone(),
                    &instruction.data,
                )
                .expect("failed to configure next instruction");

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
                    std::iter::once(instruction.data.as_ref()),
                )
            } else {
                invoke_context.process_instruction(&mut compute_units_consumed, &mut timings)
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
                                .accounts()
                                .try_borrow(index)
                                .unwrap()
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

    /// Convert this `Mollusk` instance into a `MolluskContext` for stateful
    /// testing.
    ///
    /// Creates a context wrapper that manages persistent state between
    /// instruction executions, starting with the provided account store.
    ///
    /// Process an instruction using pre-compiled instruction data and a provided TransactionContext.
    pub fn process_instruction_with_compiled_context(
        &self,
        instruction: &Instruction,
        transaction_context: &mut TransactionContext,
        instruction_accounts: Vec<InstructionAccount>,
        program_id_index: u16,
        log: Option<Rc<RefCell<LogCollector>>>,
    ) -> InstructionResult {
        let mut compute_units_consumed = 0;
        let mut timings = ExecuteTimings::default();

        let loader_key = if crate::program::precompile_keys::is_precompile(&instruction.program_id)
        {
            crate::program::loader_keys::NATIVE_LOADER
        } else if instruction.program_id == solana_system_program::id() {
            crate::program::loader_keys::NATIVE_LOADER
        } else {
            self.program_cache
                .load_program(&instruction.program_id)
                .or_panic_with(MolluskError::ProgramNotCached(&instruction.program_id))
                .account_owner()
        };

        let invoke_result = {
            let mut program_cache = self.program_cache.cache();
            let callback = MolluskInvokeContextCallback {
                epoch_stake: &self.epoch_stake,
                feature_set: &self.feature_set,
            };
            let runtime_features = self.feature_set.runtime_features();
            let sysvar_cache = self.sysvars.setup_sysvar_cache(&[]);
            let mut invoke_context = InvokeContext::new(
                transaction_context,
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

            // Configure the next instruction frame for this invocation.
            invoke_context
                .transaction_context
                .configure_next_instruction_for_tests(
                    program_id_index,
                    instruction_accounts,
                    &instruction.data,
                )
                .expect("failed to configure next instruction");

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
                    std::iter::once(instruction.data.as_ref()),
                )
            } else {
                invoke_context.process_instruction(&mut compute_units_consumed, &mut timings)
            };

            #[cfg(feature = "invocation-inspect-callback")]
            self.invocation_inspect_callback
                .after_invocation(&invoke_context);

            result
        };

        let return_data = transaction_context.get_return_data().1.to_vec();
        // For compiled context, we don't extract accounts since they're already in the context
        let resulting_accounts = vec![];

        InstructionResult {
            compute_units_consumed,
            execution_time: timings.details.execute_us.0,
            program_result: invoke_result.clone().into(),
            raw_result: invoke_result,
            return_data,
            resulting_accounts,
        }
    }

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
            account_store: Arc::new(RwLock::new(account_store)), //Rc::new(RefCell::new(account_store)),
            hydrate_store: true,                                 // <-- Default
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
    //pub account_store: Rc<RefCell<AS>>,
    pub account_store: Arc<RwLock<AS>>,
    pub hydrate_store: bool,
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
                        if account.clone() == Account::default() {
                            //let pubkeystr = pubkey.to_string();
                            //println!("Account::default {}",pubkeystr);
                            accounts.push((*pubkey, account.clone()));
                            //return;
                        } else {
                            accounts.push((*pubkey, account.clone()));
                        }
                    }
                });
        });
        accounts
    }

    fn consume_mollusk_result(&mut self, result: InstructionResult, simulated: bool) {
        let InstructionResult {
            compute_units_consumed,
            execution_time,
            program_result,
            raw_result,
            return_data,
            mut resulting_accounts,
        } = result;

        let mut store = self.account_store.write().unwrap();
        // need to add programdata accounts first if there are any
        itertools::partition(&mut resulting_accounts, |x| {
            x.1.owner == solana_sdk_ids::bpf_loader_upgradeable::id()
                && x.1.data.first().is_some_and(|byte| *byte == 3)
        });
        for (pubkey, account) in resulting_accounts {
            if account.executable
                && pubkey != Pubkey::default()
                && account.owner != solana_sdk_ids::native_loader::id()
            {
                if solana_sdk_ids::bpf_loader_upgradeable::check_id(&account.owner) {
                    let Ok(UpgradeableLoaderState::Program {
                        programdata_address,
                    }) = account.state()
                    else {
                        continue;
                    };

                    // Load the program data account to get the ELF
                    if let Some(programdata_account) = store.get_account(&programdata_address) {
                        // Extract the ELF data from the program data account
                        let elf_offset = solana_loader_v3_interface::state::UpgradeableLoaderState::size_of_programdata_metadata();
                        if programdata_account.data.len() > elf_offset {
                            let elf_data = &programdata_account.data[elf_offset..];
                            // Add the program to the cache with the ELF data
                            self.mollusk.add_program_with_elf_and_loader(
                                &pubkey,
                                elf_data,
                                &account.owner,
                            );
                        }
                    }
                }
            } else {
                if pubkey == solana_sdk_ids::sysvar::clock::id() {
                    if !account.data.is_empty() {
                        match bincode::deserialize::<solana_clock::Clock>(&account.data) {
                            Ok(parsed) => self.mollusk.set_sysvar(&parsed),
                            Err(e) => {
                                println!("Warning: Failed to deserialize clock sysvar: {:?}", e)
                            }
                        }
                    }
                }
                if pubkey == solana_sdk_ids::sysvar::rent::id() {
                    if !account.data.is_empty() {
                        match bincode::deserialize::<solana_rent::Rent>(&account.data) {
                            Ok(parsed) => self.mollusk.set_sysvar(&parsed),
                            Err(e) => {
                                println!("Warning: Failed to deserialize rent sysvar: {:?}", e)
                            }
                        }
                    }
                }
            }
            if !simulated {
                store.store_account(pubkey, account.clone());
            }
        }
    }

    /// Process an instruction using the minified Solana Virtual Machine (SVM)
    /// environment. Simply returns the result.
    pub fn process_instruction_log(
        &mut self,
        instruction: &Instruction,
        log: Option<Rc<RefCell<LogCollector>>>,
        simulated: bool,
    ) -> (
        InstructionResult,
        solana_transaction_context::TransactionContext,
    ) {
        let accounts = self.load_accounts_for_instructions(once(instruction));
        let (result, tc) = self
            .mollusk
            .process_instruction_log(instruction, &accounts, log);
        //let result = self.mollusk.process_instruction(instruction, &accounts);
        self.consume_mollusk_result(result.clone(), simulated);
        (result, tc)
    }

    /// Process a chain of instructions using the minified Solana Virtual
    /// Machine (SVM) environment.
    pub fn process_instruction_chain_log(
        &mut self,
        instructions: &[Instruction],
        log: Option<Rc<RefCell<LogCollector>>>,
        simulated: bool,
    ) -> (
        InstructionResult,
        solana_transaction_context::TransactionContext,
    ) {
        let mut last_result = InstructionResult {
            compute_units_consumed: 0,
            execution_time: 0,
            program_result: Ok(()).into(),
            raw_result: Ok(()),
            return_data: vec![],
            resulting_accounts: vec![],
        };
        let mut last_tc = solana_transaction_context::TransactionContext::new(
            vec![],
            solana_rent::Rent::default(),
            0,
            0,
        );
        for instruction in instructions {
            let (result, tc) = self.process_instruction_log(instruction, log.clone(), simulated);
            last_result = result;
            last_tc = tc;
            if !last_result.program_result.is_ok() {
                break;
            }
        }
        (last_result, last_tc)
    }

    /// Process a transaction with multiple instructions using a shared TransactionContext.
    pub fn process_tx(
        &mut self,
        instructions: &[Instruction],
        log: Option<Rc<RefCell<LogCollector>>>,
        simulated: bool,
    ) -> (Vec<InstructionResult>, TransactionContext) {
        // Load all accounts needed for all instructions first
        let all_accounts = self.load_accounts_for_instructions(instructions.iter());

        // Create a closure that can fetch accounts from our loaded accounts
        let account_getter = |pubkey: &Pubkey| {
            all_accounts
                .iter()
                .find(|(k, _)| k == pubkey)
                .map(|(_, account)| account.clone())
        };

        // Compile transaction accounts using the store-based approach for multiple instructions
        let key_map = KeyMap::compile_from_instructions(instructions.iter());
        let transaction_accounts = compile_transaction_accounts_from_store(
            &key_map,
            instructions,
            &account_getter,
            Some(Box::new(|| {
                let mut program_account = Account::default();
                program_account.set_owner(crate::program::loader_keys::NATIVE_LOADER);
                program_account.set_executable(true);
                program_account
            })),
        );

        let mut transaction_context = TransactionContext::new(
            transaction_accounts,
            self.mollusk.sysvars.rent.clone(),
            self.mollusk.compute_budget.max_instruction_stack_depth,
            self.mollusk.compute_budget.max_instruction_trace_length,
        );

        let mut results = Vec::new();
        for instruction in instructions {
            // Use the same key_map for all instructions
            let compiled_instruction = compile_instruction_without_data(&key_map, instruction);
            let instruction_accounts =
                compile_instruction_accounts(&key_map, &compiled_instruction);
            let program_id_index = compiled_instruction.program_id_index as u16;

            let result = self.mollusk.process_instruction_with_compiled_context(
                instruction,
                &mut transaction_context,
                instruction_accounts,
                program_id_index,
                log.clone(),
            );

            results.push(result.clone());

            // Update account state after each successful instruction
            if result.program_result.is_ok() {
                if !simulated {
                    let mut store = self.account_store.write().unwrap();
                    for account_meta in &instruction.accounts {
                        if let Some(index) =
                            transaction_context.find_index_of_account(&account_meta.pubkey)
                        {
                            if let Ok(context_account) =
                                transaction_context.accounts().try_borrow(index)
                            {
                                store.store_account(
                                    account_meta.pubkey,
                                    (*context_account).clone().into(),
                                );
                            }
                        }
                    }
                }
            } else {
                // If instruction fails, stop processing
                break;
            }
        }

        (results, transaction_context)
    }
}
