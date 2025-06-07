//! Agave SVM implementation for Mollusk.
//!
//! This crate provides an `AgaveSVM` struct that implements the `SVM` trait
//! using Agave's program runtime components.

use {
    agave_feature_set::FeatureSet,
    agave_precompiles::get_precompile,
    mollusk_svm_agave_programs::ProgramCache,
    mollusk_svm_agave_sysvars::Sysvars,
    mollusk_svm_error::error::{MolluskError, MolluskPanic},
    solana_account::Account,
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_hash::Hash,
    solana_instruction::{error::InstructionError, Instruction},
    solana_log_collector::LogCollector,
    solana_program_runtime::invoke_context::{EnvironmentConfig, InvokeContext},
    solana_pubkey::Pubkey,
    solana_timings::ExecuteTimings,
    solana_transaction_context::{InstructionAccount, TransactionContext},
    std::{cell::RefCell, rc::Rc, sync::Arc},
};

#[derive(Default)]
pub struct AgaveSVM {
    pub program_cache: ProgramCache,
    pub sysvars: Sysvars,
}

impl AgaveSVM {
    pub fn get_loader_key(&self, program_id: &Pubkey) -> Pubkey {
        if mollusk_svm_agave_programs::precompile_keys::is_precompile(program_id) {
            mollusk_svm_agave_programs::loader_keys::NATIVE_LOADER
        } else {
            // [VM]: The program cache is really only required to use the
            // Agave program-runtime API.
            self.program_cache
                .load_program(program_id)
                .or_panic_with(MolluskError::ProgramNotCached(program_id))
                .account_owner()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process_instruction(
        &self,
        instruction: &Instruction,
        accounts: &[(Pubkey, Account)],
        instruction_accounts: &[InstructionAccount],
        transaction_context: &mut TransactionContext,
        program_id_index: u16,
        compute_budget: ComputeBudget,
        feature_set: Arc<FeatureSet>,
        lamports_per_signature: u64,
        logger: Option<Rc<RefCell<LogCollector>>>,
        compute_units_consumed: &mut u64,
        timings: &mut ExecuteTimings,
    ) -> Result<(), InstructionError> {
        let mut program_cache = self.program_cache.cache();
        let sysvar_cache = self.sysvars.setup_sysvar_cache(accounts);

        let mut invoke_context = InvokeContext::new(
            transaction_context,
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
        if let Some(precompile) = get_precompile(&instruction.program_id, |feature_id| {
            invoke_context.get_feature_set().is_active(feature_id)
        }) {
            invoke_context.process_precompile(
                precompile,
                &instruction.data,
                instruction_accounts,
                &[program_id_index],
                std::iter::once(instruction.data.as_ref()),
            )
        } else {
            invoke_context.process_instruction(
                &instruction.data,
                instruction_accounts,
                &[program_id_index],
                compute_units_consumed,
                timings,
            )
        }
    }
}
