//! Instruction <-> Transaction account compilation, with key deduplication,
//! privilege handling, and program account stubbing.

use {
    mollusk_svm_keys::{
        accounts::{
            compile_instruction_accounts, compile_instruction_without_data,
            compile_transaction_accounts_for_instruction_from_store,
        },
        keys::KeyMap,
    },
    solana_account::{Account, WritableAccount},
    solana_instruction::Instruction,
    solana_pubkey::Pubkey,
    solana_transaction_context::{InstructionAccount, TransactionAccount},
    crate::account_store::AccountStore,
};

pub struct CompiledAccounts {
    pub program_id_index: u16,
    pub instruction_accounts: Vec<InstructionAccount>,
    pub transaction_accounts: Vec<TransactionAccount>,
}

pub fn compile_accounts(
    instruction: &Instruction,
    accounts: &[(Pubkey, Account)],
    loader_key: Pubkey,
) -> CompiledAccounts {
    let stub_out_program_account = move || {
        let mut program_account = Account::default();
        program_account.set_owner(loader_key);
        program_account.set_executable(true);
        program_account
    };

    let key_map = KeyMap::compile_from_instruction(instruction);
    let compiled_instruction = compile_instruction_without_data(&key_map, instruction);
    let instruction_accounts = compile_instruction_accounts(&key_map, &compiled_instruction);
    let transaction_accounts = compile_transaction_accounts_for_instruction_from_store(
        &key_map,
        instruction,
        &|pubkey: &Pubkey| {
            accounts
                .iter()
                .find(|(k, _)| k == pubkey)
                .map(|(_, account)| account.clone())
        },
        Some(Box::new(stub_out_program_account)),
    );

    CompiledAccounts {
        program_id_index: compiled_instruction.program_id_index as u16,
        instruction_accounts,
        transaction_accounts,
    }
}

/// Compile accounts for an instruction using accounts from an AccountStore.
/// This function fetches accounts from the account_store instead of using a pre-provided slice.
pub fn compile_accounts_from_store<AS: AccountStore>(
    instruction: &Instruction,
    account_store: &AS,
    loader_key: Pubkey,
) -> CompiledAccounts {
    let stub_out_program_account = move || {
        let mut program_account = Account::default();
        program_account.set_owner(loader_key);
        program_account.set_executable(true);
        program_account
    };

    let key_map = KeyMap::compile_from_instruction(instruction);
    let compiled_instruction = compile_instruction_without_data(&key_map, instruction);
    let instruction_accounts = compile_instruction_accounts(&key_map, &compiled_instruction);

    // Create a closure that wraps the AccountStore's get_account method
    let account_getter = |pubkey: &Pubkey| account_store.get_account(pubkey);

    let transaction_accounts = compile_transaction_accounts_for_instruction_from_store(
        &key_map,
        instruction,
        &account_getter,
        Some(Box::new(stub_out_program_account)),
    );

    CompiledAccounts {
        program_id_index: compiled_instruction.program_id_index as u16,
        instruction_accounts,
        transaction_accounts,
    }
}
