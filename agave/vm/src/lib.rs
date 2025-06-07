//! Agave SVM implementation for Mollusk.
//!
//! This crate provides an `AgaveSVM` struct that implements the `SVM` trait
//! using Agave's program runtime components.

use {
    agave_feature_set::FeatureSet,
    agave_precompiles::get_precompile,
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_hash::Hash,
    solana_instruction::{error::InstructionError, Instruction},
    solana_log_collector::LogCollector,
    solana_program_runtime::{
        invoke_context::{EnvironmentConfig, InvokeContext},
        loaded_programs::ProgramCacheForTxBatch,
        sysvar_cache::SysvarCache,
    },
    solana_timings::ExecuteTimings,
    solana_transaction_context::{InstructionAccount, TransactionContext},
    std::{cell::RefCell, rc::Rc, sync::Arc},
};

pub struct AgaveSVM;

impl AgaveSVM {
    #[allow(clippy::too_many_arguments)]
    pub fn process_instruction(
        instruction: &Instruction,
        instruction_accounts: &[InstructionAccount],
        transaction_context: &mut TransactionContext,
        program_cache: &mut ProgramCacheForTxBatch,
        sysvar_cache: &SysvarCache,
        program_id_index: u16,
        compute_budget: ComputeBudget,
        feature_set: Arc<FeatureSet>,
        lamports_per_signature: u64,
        logger: Option<Rc<RefCell<LogCollector>>>,
        compute_units_consumed: &mut u64,
        timings: &mut ExecuteTimings,
    ) -> Result<(), InstructionError> {
        let mut invoke_context = InvokeContext::new(
            transaction_context,
            program_cache,
            EnvironmentConfig::new(
                Hash::default(),
                lamports_per_signature,
                0,
                &|_| 0,
                feature_set,
                sysvar_cache,
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
