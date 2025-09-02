use {
    mollusk_svm::mt::MolluskMt,
    solana_account::Account,
    solana_log_collector::LogCollector,
    solana_pubkey::Pubkey,
    solana_system_interface::instruction::transfer,
    solana_system_program,
    std::{cell::RefCell, collections::HashMap, rc::Rc},
};

#[test]
fn test_process_instruction_chain_log_basic() {
    let mollusk = MolluskMt::default();
    
    // Create a simple transfer instruction
    let payer_pubkey = Pubkey::new_unique();
    let recipient_pubkey = Pubkey::new_unique();
    
    let payer_account = Account {
        lamports: 100_000_000, // 0.1 SOL
        data: vec![],
        owner: solana_system_program::id(),
        executable: false,
        rent_epoch: 0,
    };
    
    let recipient_account = Account {
        lamports: 0,
        data: vec![],
        owner: solana_system_program::id(),
        executable: false,
        rent_epoch: 0,
    };
    
    // Create a context
    let mut context = mollusk.with_context(HashMap::new());
    
    // Store accounts in the context's account store
    {
        let mut store = context.account_store.write().unwrap();
        store.insert(payer_pubkey, payer_account);
        store.insert(recipient_pubkey, recipient_account);
    }
    
    // Create transfer instruction
    let instruction = transfer(&payer_pubkey, &recipient_pubkey, 1_000_000); // 0.001 SOL
    
    // Create a log collector
    let log_collector = Some(Rc::new(RefCell::new(LogCollector {
        messages: Vec::new(),
        bytes_written: 0,
        bytes_limit: Some(1000),
        limit_warning: false,
    })));
    
    // Process the instruction chain with log using the context
    let (result, _transaction_context) = context.process_instruction_chain_log(
        &[instruction],
        log_collector,
        false, // simulated
    );
    
    // Print some information about the result
    println!("Compute units consumed: {}", result.compute_units_consumed);
    println!("Execution time: {}", result.execution_time);
    println!("Program result: {:?}", result.program_result);
    
    // Check that accounts were handled correctly
    let store = context.account_store.read().unwrap();
    if let Some(account) = store.get(&payer_pubkey) {
        println!("Payer final lamports: {}", account.lamports);
    }
    if let Some(account) = store.get(&recipient_pubkey) {
        println!("Recipient final lamports: {}", account.lamports);
    }
}