use {
    mollusk_svm::{
        result::Check,
        Mollusk,
    },
    solana_account::Account,
    solana_instruction::{AccountMeta, Instruction},
    solana_pubkey::Pubkey,
};

#[test]
fn test_entrypoint_register_metadata() {
    std::env::set_var("SBF_OUT_DIR", "../target/deploy");
    let program_id = Pubkey::new_unique();

    let mut mollusk = Mollusk::new(&program_id, "entrypoint_metadata_program");
    
    // Enable the feature for entrypoint metadata in registers 2 & 3.
    mollusk
        .feature_set
        .activate(&agave_feature_set::additional_entrypoint_metadata_in_vm_registers::id(), 0);

    let key = Pubkey::new_unique();
    let account = Account::new(1_000, 16, &program_id);

    // Test various combinations of account counts and instruction data lengths
    for (num_accounts, instruction_data_len) in [(1, 0), (2, 5), (3, 10), (5, 20)] {
        // The first account's data should reflect the metadata passed in
        // registers 2 & 3.
        let expected_data = {
            let mut data = vec![0u8; 16];
            data[0..8].copy_from_slice(&(num_accounts as u64).to_le_bytes());
            data[8..16].copy_from_slice(&(instruction_data_len as u64).to_le_bytes());
            data
        };

        let additional_accounts = if num_accounts > 1 { num_accounts - 1 } else { 0 };
        let keys = vec![Pubkey::new_unique(); additional_accounts];
        let input_data = vec![4; instruction_data_len];

        let mut metas = vec![AccountMeta::new(key, false)];
        let mut accounts = vec![(key, account.clone())];
        for k in keys.iter() {
            metas.push(AccountMeta::new_readonly(*k, false));
            accounts.push((*k, Account::default()));
        }

        mollusk.process_and_validate_instruction(
            &Instruction::new_with_bytes(program_id, &input_data, metas),
            &accounts,
            &[
                Check::success(),
                Check::account(&key)
                    .data(&expected_data)
                    .build(),
            ],
        );
    }
}
