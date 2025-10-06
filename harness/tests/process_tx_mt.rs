use {
    mollusk_svm::mt::MolluskMt,
    solana_account::{Account, ReadableAccount},
    solana_pubkey::Pubkey,
    solana_svm_log_collector::LogCollector,
    solana_system_interface::instruction as system_instruction,
    std::{cell::RefCell, collections::HashMap, rc::Rc},
};

#[test]
fn test_process_tx_multiple_transfers() {
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

    let mut context = mollusk.with_context(account_store);

    // Create instructions for the transaction
    let instructions = vec![
        system_instruction::transfer(&alice, &bob, transfer1_amount),
        system_instruction::transfer(&bob, &charlie, transfer2_amount),
    ];

    // Process the transaction with shared context
    let log_collector = LogCollector {
        bytes_limit: Some(10_000),
        ..Default::default()
    };
    let log = Some(Rc::new(RefCell::new(log_collector)));
    let (results, _transaction_context) = context.process_tx(&instructions, log, false);

    // Verify results
    assert_eq!(results.len(), 2);
    for (i, result) in results.iter().enumerate() {
        if !result.program_result.is_ok() {
            println!("Result {} failed: {:?}", i, result.program_result);
        }
        assert!(result.program_result.is_ok());
    }

    // Verify final account states
    let store = context.account_store.read().unwrap();

    let alice_account = store.get(&alice).unwrap();
    assert_eq!(
        alice_account.lamports,
        initial_lamports - transfer1_amount
    );

    let bob_account = store.get(&bob).unwrap();
    assert_eq!(
        bob_account.lamports,
        initial_lamports + transfer1_amount - transfer2_amount
    );

    let charlie_account = store.get(&charlie).unwrap();
    assert_eq!(
        charlie_account.lamports,
        initial_lamports + transfer2_amount
    );
}

#[test]
fn test_process_tx_with_failure() {
    let sender = Pubkey::new_unique();
    let recipient = Pubkey::new_unique();

    let base_lamports = 100_000u64;
    let transfer_amount = 200_000u64; // More than available

    // Create context
    let mollusk = MolluskMt::default();
    let mut account_store = HashMap::new();

    account_store.insert(
        sender,
        Account::new(base_lamports, 0, &solana_sdk_ids::system_program::id()),
    );
    account_store.insert(
        recipient,
        Account::new(base_lamports, 0, &solana_sdk_ids::system_program::id()),
    );

    let mut context = mollusk.with_context(account_store);

    // Create instructions: first succeeds, second fails
    let instructions = vec![
        system_instruction::transfer(&sender, &recipient, 50_000),
        system_instruction::transfer(&sender, &recipient, transfer_amount), // Should fail
    ];

    let (results, _transaction_context) = context.process_tx(&instructions, None, false);

    // Verify results
    assert_eq!(results.len(), 2);
    assert!(results[0].program_result.is_ok());
    assert!(results[1].program_result.is_err());

    // Verify account states (only first transfer should have happened)
    let store = context.account_store.read().unwrap();

    let sender_account = store.get(&sender).unwrap();
    assert_eq!(sender_account.lamports, base_lamports - 50_000);

    let recipient_account = store.get(&recipient).unwrap();
    assert_eq!(recipient_account.lamports, base_lamports + 50_000);
}

#[test]
fn test_process_tx_simulated() {
    let alice = Pubkey::new_unique();
    let bob = Pubkey::new_unique();

    let initial_lamports = 500_000u64;
    let transfer_amount = 100_000u64;

    // Create context with HashMap account store
    let mollusk = MolluskMt::default();
    let mut account_store = HashMap::new();

    account_store.insert(
        alice,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );
    account_store.insert(
        bob,
        Account::new(initial_lamports, 0, &solana_sdk_ids::system_program::id()),
    );

    let mut context = mollusk.with_context(account_store);

    // Create instructions for the transaction
    let instructions = vec![
        system_instruction::transfer(&alice, &bob, transfer_amount),
    ];

    // Process the transaction in simulation mode (should not update accounts)
    let (results, _transaction_context) = context.process_tx(&instructions, None, true);

    // Verify results
    assert_eq!(results.len(), 1);
    assert!(results[0].program_result.is_ok());

    // Verify account states remain unchanged (since it was simulated)
    {
        let store = context.account_store.read().unwrap();

        let alice_account = store.get(&alice).unwrap();
        assert_eq!(alice_account.lamports, initial_lamports); // Should be unchanged

        let bob_account = store.get(&bob).unwrap();
        assert_eq!(bob_account.lamports, initial_lamports); // Should be unchanged
    }

    // Now process the same transaction normally (should update accounts)
    let (results, _transaction_context) = context.process_tx(&instructions, None, false);

    // Verify results
    assert_eq!(results.len(), 1);
    assert!(results[0].program_result.is_ok());

    // Verify account states are now updated
    {
        let store = context.account_store.read().unwrap();

        let alice_account = store.get(&alice).unwrap();
        assert_eq!(alice_account.lamports, initial_lamports - transfer_amount);

        let bob_account = store.get(&bob).unwrap();
        assert_eq!(bob_account.lamports, initial_lamports + transfer_amount);
    }
}