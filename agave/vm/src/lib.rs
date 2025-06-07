//! Agave SVM implementation for Mollusk.
//!
//! This crate provides an `AgaveSVM` struct that implements the `SVM` trait
//! using Agave's program runtime components.

use {
    agave_feature_set::FeatureSet,
    agave_precompiles::get_precompile,
    mollusk_svm_agave_programs::ProgramCache,
    mollusk_svm_agave_sysvars::Sysvars,
    mollusk_svm_compile_accounts::{compile_accounts, CompiledAccounts},
    mollusk_svm_error::error::{MolluskError, MolluskPanic},
    solana_account::Account,
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_hash::Hash,
    solana_instruction::{error::InstructionError, Instruction},
    solana_log_collector::LogCollector,
    solana_program_runtime::invoke_context::{EnvironmentConfig, InvokeContext},
    solana_pubkey::Pubkey,
    solana_timings::ExecuteTimings,
    solana_transaction_context::TransactionContext,
    std::{cell::RefCell, rc::Rc, sync::Arc},
};

#[derive(Default)]
pub struct AgaveSVM {
    pub program_cache: ProgramCache,
    pub sysvars: Sysvars,
}

impl AgaveSVM {
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::type_complexity)]
    pub fn process_instruction(
        &self,
        instruction: &Instruction,
        accounts: &[(Pubkey, Account)],
        compute_budget: ComputeBudget,
        feature_set: Arc<FeatureSet>,
        lamports_per_signature: u64,
        logger: Option<Rc<RefCell<LogCollector>>>,
        compute_units_consumed: &mut u64,
        timings: &mut ExecuteTimings,
    ) -> (
        Result<(), InstructionError>,
        Vec<u8>,
        Vec<(Pubkey, Account)>,
    ) {
        let loader_key = if mollusk_svm_agave_programs::precompile_keys::is_precompile(
            &instruction.program_id,
        ) {
            mollusk_svm_agave_programs::loader_keys::NATIVE_LOADER
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
        } = compile_accounts(instruction, accounts, loader_key);

        let mut transaction_context = TransactionContext::new(
            transaction_accounts,
            self.sysvars.rent.clone(),
            compute_budget.max_instruction_stack_depth,
            compute_budget.max_instruction_trace_length,
        );

        let mut program_cache = self.program_cache.cache();
        let sysvar_cache = self.sysvars.setup_sysvar_cache(accounts);

        let mut invoke_context = InvokeContext::new(
            &mut transaction_context,
            &mut program_cache,
            EnvironmentConfig::new(
                Hash::default(),
                lamports_per_signature,
                0,
                &|_| 0,
                feature_set,
                &sysvar_cache,
            ),
            logger,
            compute_budget,
        );

        let invoke_result = if let Some(precompile) =
            get_precompile(&instruction.program_id, |feature_id| {
                invoke_context.get_feature_set().is_active(feature_id)
            }) {
            invoke_context.process_precompile(
                precompile,
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
                compute_units_consumed,
                timings,
            )
        };

        // [VM]: This should be a required output in the interface.
        let return_data = transaction_context.get_return_data().1.to_vec();

        // [VM]: This should still be done in here, since the concept of a
        // "resulting account" and how it's compared to the inputs is a Mollusk
        // API thing.
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

        (invoke_result, return_data, resulting_accounts)
    }
}
