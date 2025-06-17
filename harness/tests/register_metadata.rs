use {
    mollusk_svm::{Mollusk, result::Check},
    solana_account::Account,
    solana_instruction::{AccountMeta, Instruction},
    solana_pubkey::Pubkey,
};

#[test]
fn test_entrypoint_register_metadata() {
    std::env::set_var("SBF_OUT_DIR", "../target/deploy");
    
    let program_id = Pubkey::new_unique();

    let mut mollusk = Mollusk::new(&program_id, "entrypoint_metadata_program");
    
    // Enable the feature for entrypoint metadata in register 2.
    mollusk
        .feature_set
        .activate(&agave_feature_set::additional_entrypoint_metadata_in_vm_registers::id(), 0);

    // Test various input buffers.
    for input_data in [
        vec![1, 2, 3, 4, 5],
        vec![6; 16],
        vec![7; 32],
        vec![8; 64],
    ] {
        let key = Pubkey::new_unique();
        let account = {
            let space = input_data.len();
            let lamports = mollusk.sysvars.rent.minimum_balance(space);
            Account::new(lamports, space, &program_id)
        };

        mollusk.process_and_validate_instruction(
            &Instruction::new_with_bytes(program_id, &input_data, vec![AccountMeta::new(key, false)]),
            &[(key, account)],
            &[
                Check::success(),
                Check::account(&key)
                    .data(&input_data)
                    .build(),
            ]
        );
    }
}
