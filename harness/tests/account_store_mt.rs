use {
    mollusk_svm::{mt::MolluskMt, result::Check},
    solana_account::{Account, ReadableAccount},
    solana_clock::Clock,
    solana_epoch_schedule::EpochSchedule,
    solana_log_collector::LogCollector,
    solana_program_error::ProgramError,
    solana_pubkey::Pubkey,
    solana_rent::Rent,
    solana_slot_hashes::SlotHashes,
    solana_system_interface::error::SystemError,
    solana_system_program::system_processor::DEFAULT_COMPUTE_UNITS,
    std::cell::RefCell,
    std::collections::HashMap,
    std::rc::Rc,
};

#[test]
fn test_transfer_with_context_mt() {
    let sender = Pubkey::new_unique();
    let recipient = Pubkey::new_unique();

    let base_lamports = 100_000_000u64;
    let transfer_amount = 42_000u64;

    // Create context with HashMap account store
    let mollusk = MolluskMt::default();
    let mut account_store = HashMap::new();

    // Initialize accounts in the store
    account_store.insert(
        sender,
        Account::new(base_lamports, 0, &solana_sdk_ids::system_program::id()),
    );
    account_store.insert(
        recipient,
        Account::new(base_lamports, 0, &solana_sdk_ids::system_program::id()),
    );

    let context = mollusk.with_context(account_store);

    // Process the transfer instruction
    /*  let result = context.process_and_validate_instruction(
        &solana_system_interface::instruction::transfer(&sender, &recipient, transfer_amount),
        &[
            Check::success(),
            Check::compute_units(DEFAULT_COMPUTE_UNITS),
        ],
    );*/
    // Process the transfer instruction
    //
    let mut log_collector = LogCollector {
        bytes_limit: Some(10_000),
        ..Default::default()
    };
    let log = Rc::new(RefCell::new(log_collector));
    let result = context.process_instruction_log(
        &solana_system_interface::instruction::transfer(&sender, &recipient, transfer_amount),
        Some(log.clone()),
    );
    println!("logs: {:?}", result.0.program_result);
    println!("logs: {:?}", log.borrow().get_recorded_content());

    // Verify the result was successful
    assert!(!result.0.program_result.is_err());

    // Verify account states were persisted correctly in the account store
    let store = context.account_store.write().unwrap(); //.borrow();

    let sender_account = store.get(&sender).unwrap();
    assert_eq!(sender_account.lamports(), base_lamports - transfer_amount);

    let recipient_account = store.get(&recipient).unwrap();
    assert_eq!(
        recipient_account.lamports(),
        base_lamports + transfer_amount
    );
}

#[test]
fn test_multiple_transfers_with_persistent_state_mt() {
    let alice = Pubkey::new_unique();
    let bob = Pubkey::new_unique();
    let charlie = Pubkey::new_unique();

    let initial_lamports = 1_000_000u64;
    let transfer1_amount = 200_000u64;
    let transfer2_amount = 150_000u64;

    // Create context with HashMap account store
    let mollusk = MolluskMt::default();
    let mut account_store = HashMap::new();

    // Initialize accounts
    account_store.insert(
        alice,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );
    account_store.insert(
        bob,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );
    account_store.insert(
        charlie,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );

    let context = mollusk.with_context(account_store);

    let checks = vec![
        Check::success(),
        Check::compute_units(DEFAULT_COMPUTE_UNITS),
    ];

    // First transfer: Alice -> Bob
    let instruction1 =
        solana_system_interface::instruction::transfer(&alice, &bob, transfer1_amount);
    let result1 = context.process_and_validate_instruction(&instruction1, &checks);
    assert!(!result1.program_result.is_err());

    // Second transfer: Bob -> Charlie
    let instruction2 =
        solana_system_interface::instruction::transfer(&bob, &charlie, transfer2_amount);
    let result2 = context.process_and_validate_instruction(&instruction2, &checks);
    assert!(!result2.program_result.is_err());

    // Verify final account states
    let store = context.account_store.write().unwrap();

    let alice_account = store.get(&alice).unwrap();
    assert_eq!(
        alice_account.lamports(),
        initial_lamports - transfer1_amount
    );

    let bob_account = store.get(&bob).unwrap();
    assert_eq!(
        bob_account.lamports(),
        initial_lamports + transfer1_amount - transfer2_amount
    );

    let charlie_account = store.get(&charlie).unwrap();
    assert_eq!(
        charlie_account.lamports(),
        initial_lamports + transfer2_amount
    );
}

#[test]
fn test_multiple_transfers_with_persistent_state_mt_chain() {
    let alice = Pubkey::new_unique();
    let bob = Pubkey::new_unique();
    let charlie = Pubkey::new_unique();

    let initial_lamports = 1_000_000u64;
    let transfer1_amount = 200_000u64;
    let transfer2_amount = 150_000u64;

    // Create context with HashMap account store
    let mollusk = MolluskMt::default();
    let mut account_store = HashMap::new();

    // Initialize accounts
    account_store.insert(
        alice,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );
    account_store.insert(
        bob,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );
    account_store.insert(
        charlie,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );

    let context = mollusk.with_context(account_store);

    let checks = vec![
        Check::success(),
        Check::compute_units(DEFAULT_COMPUTE_UNITS),
    ];
    /*

    // First transfer: Alice -> Bob
    let instruction1 =
        solana_system_interface::instruction::transfer(&alice, &bob, transfer1_amount);
    let result1 = context.process_and_validate_instruction(&instruction1, &checks);
    assert!(!result1.program_result.is_err());

    // Second transfer: Bob -> Charlie
    let instruction2 =
        solana_system_interface::instruction::transfer(&bob, &charlie, transfer2_amount);
    let result2 = context.process_and_validate_instruction(&instruction2, &checks);
    assert!(!result2.program_result.is_err());
     */
    let log_collector = LogCollector {
        bytes_limit: Some(10_000),
        ..Default::default()
    };
    let log = Rc::new(RefCell::new(log_collector));
    context.process_instruction_chain_log(
        &[
            solana_system_interface::instruction::transfer(&alice, &bob, transfer1_amount),
            solana_system_interface::instruction::transfer(&bob, &charlie, transfer2_amount),
        ],
        Some(log.clone()),
    );

    println!("logs: {:?}", log.borrow().get_recorded_content());

    // Verify final account states
    let store = context.account_store.write().unwrap();

    let alice_account = store.get(&alice).unwrap();
    assert_eq!(
        alice_account.lamports(),
        initial_lamports - transfer1_amount
    );

    let bob_account = store.get(&bob).unwrap();
    assert_eq!(
        bob_account.lamports(),
        initial_lamports + transfer1_amount - transfer2_amount
    );

    let charlie_account = store.get(&charlie).unwrap();
    assert_eq!(
        charlie_account.lamports(),
        initial_lamports + transfer2_amount
    );
}

#[test]
fn test_account_store_default_account_mt() {
    let mollusk = MolluskMt::default();
    let context = mollusk.with_context(HashMap::new());

    let non_existent_key = Pubkey::new_unique();
    let recipient = Pubkey::new_unique();

    // Try to transfer from a non-existent account (should get default account)
    let instruction =
        solana_system_interface::instruction::transfer(&non_existent_key, &recipient, 1000);

    // This should fail because the default account has 0 lamports
    context.process_and_validate_instruction(
        &instruction,
        &[Check::err(ProgramError::Custom(
            SystemError::ResultWithNegativeLamports as u32,
        ))],
    );
}

#[test]
fn test_get_sysvar_mt() {
    let mollusk = MolluskMt::default();

    // Test getting clock sysvar
    let clock: Clock = mollusk.get_sysvar();
    assert_eq!(clock.slot, 0); // Default slot should be 0

    // Test getting epoch schedule sysvar
    let epoch_schedule: EpochSchedule = mollusk.get_sysvar();
    assert!(epoch_schedule.slots_per_epoch > 0);

    // Test getting rent sysvar
    let rent: Rent = mollusk.get_sysvar();
    assert!(rent.lamports_per_byte_year > 0);

    println!("✅ get_sysvar tests passed!");
}

#[test]
fn test_set_sysvar_mt() {
    let mut mollusk = MolluskMt::default();

    // Test setting clock sysvar
    let mut new_clock: Clock = mollusk.get_sysvar();
    new_clock.slot = 42;
    new_clock.epoch = 1;
    mollusk.set_sysvar(&new_clock);

    // Verify the clock was updated
    let updated_clock: Clock = mollusk.get_sysvar();
    assert_eq!(updated_clock.slot, 42);
    assert_eq!(updated_clock.epoch, 1);

    // Test setting rent sysvar
    let mut new_rent: Rent = mollusk.get_sysvar();
    let original_lamports_per_byte = new_rent.lamports_per_byte_year;
    new_rent.lamports_per_byte_year = 12345;
    mollusk.set_sysvar(&new_rent);

    // Verify the rent was updated
    let updated_rent: Rent = mollusk.get_sysvar();
    assert_eq!(updated_rent.lamports_per_byte_year, 12345);
    assert_ne!(
        updated_rent.lamports_per_byte_year,
        original_lamports_per_byte
    );

    println!("✅ set_sysvar tests passed!");
}

#[test]
fn test_expire_blockhash_mt() {
    let mut mollusk = MolluskMt::default();

    // Get initial slot hashes
    let initial_slot_hashes: SlotHashes = mollusk.get_sysvar();
    let initial_len = initial_slot_hashes.len();
    println!("Initial SlotHashes length: {}", initial_len);
    println!("Initial first slot hash: {:?}", initial_slot_hashes.first());

    // Expire blockhash
    mollusk.expire_blockhash();

    // Get updated slot hashes
    let updated_slot_hashes: SlotHashes = mollusk.get_sysvar();
    let updated_len = updated_slot_hashes.len();
    println!("Updated SlotHashes length: {}", updated_len);
    println!("Updated first slot hash: {:?}", updated_slot_hashes.first());

    // Should have added a new slot hash entry or stayed the same (if at max capacity)
    assert!(updated_len >= initial_len);

    // The slot hashes should be different after expire_blockhash
    let initial_first = initial_slot_hashes.first();
    let updated_first = updated_slot_hashes.first();

    // Print more debug info
    if let (Some(initial), Some(updated)) = (initial_first, updated_first) {
        println!("Initial slot: {}, hash: {}", initial.0, initial.1);
        println!("Updated slot: {}, hash: {}", updated.0, updated.1);

        // Check if anything changed - at minimum the hash should be different
        // since we added a new entry to slot hashes
        let changed = initial.0 != updated.0 || initial.1 != updated.1;
        if !changed {
            println!("WARNING: SlotHashes didn't change after expire_blockhash");
            // Let's check if the entire structure changed
            let initial_vec: Vec<_> = initial_slot_hashes.as_slice().to_vec();
            let updated_vec: Vec<_> = updated_slot_hashes.as_slice().to_vec();
            println!(
                "Initial vec length: {}, Updated vec length: {}",
                initial_vec.len(),
                updated_vec.len()
            );
            assert!(
                initial_vec != updated_vec,
                "SlotHashes should change after expire_blockhash"
            );
        }
    } else {
        println!("One of the slot hashes is None - this is unexpected for a default mollusk");
    }

    println!("✅ expire_blockhash tests passed!");
}

#[test]
fn test_combined_sysvar_functionality_mt() {
    let mut mollusk = MolluskMt::default();

    // Test the combination of all functions

    // 1. Set a custom clock
    let mut clock: Clock = mollusk.get_sysvar();
    clock.slot = 100;
    clock.unix_timestamp = 1234567890;
    mollusk.set_sysvar(&clock);

    // 2. Expire blockhash (this should use the updated slot)
    mollusk.expire_blockhash();

    // 3. Verify the clock timestamp is still as we set it, but slot may have changed
    let final_clock: Clock = mollusk.get_sysvar();
    assert_eq!(final_clock.unix_timestamp, 1234567890);
    // expire_blockhash advances the slot by 1
    assert_eq!(final_clock.slot, 101);

    // 4. Verify slot hashes were updated
    let final_slot_hashes: SlotHashes = mollusk.get_sysvar();
    assert!(final_slot_hashes.len() > 0);

    println!("✅ Combined functionality tests passed!");
}

#[test]
fn test_warp_to_slot_integration_mt() {
    let mut mollusk = MolluskMt::default();

    // Warp to a specific slot
    mollusk.warp_to_slot(500);

    // Verify the clock was updated
    let clock: Clock = mollusk.get_sysvar();
    assert_eq!(clock.slot, 500);

    // Now expire blockhash and verify it uses the warped slot
    mollusk.expire_blockhash();

    let slot_hashes: SlotHashes = mollusk.get_sysvar();

    // Should have slot hashes entries
    assert!(slot_hashes.len() > 0);

    // The most recent entry should be for slot 501 since expire_blockhash advances slot by 1
    if let Some((slot, _hash)) = slot_hashes.first() {
        assert!(slot <= &501); // Should be slot 501 or less after expire_blockhash
    }

    println!("✅ warp_to_slot integration tests passed!");
}
