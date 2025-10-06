use {
    mollusk_svm::mt::MolluskMt,
    solana_account::{Account, state_traits::StateMut},
    solana_svm_log_collector::LogCollector,
    solana_loader_v3_interface::{
        get_program_data_address,
        instruction as loader_v3_instruction,
        state::UpgradeableLoaderState,
    },
    solana_pubkey::Pubkey,
    solana_system_program,
    std::{cell::RefCell, collections::HashMap, env, fs, path::Path, rc::Rc},
};

#[test]
fn test_deploy_solana_program_with_sbf_out_dir() {
    println!("=== Starting test_deploy_solana_program_with_sbf_out_dir ===");

    // Check if the SO file exists in various locations
    let possible_paths = vec![
        "target/deploy/test_program_primary.so",
        "target/sbpf-solana-solana/release/test_program_primary.so",
        "tests/fixtures/test_program_primary.so",
    ];

    println!("Checking for SO file in possible locations:");
    let mut found_file = false;
    for path in &possible_paths {
        if Path::new(path).exists() {
            println!("  ✓ Found file at: {}", path);
            found_file = true;
        } else {
            println!("  ✗ File not found at: {}", path);
        }
    }

    if !found_file {
        println!("  Checking current directory contents:");
        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries {
                if let Ok(entry) = entry {
                    println!("    {:?}", entry.file_name());
                }
            }
        }

        if let Ok(entries) = fs::read_dir("target/deploy") {
            println!("  Checking target/deploy contents:");
            for entry in entries {
                if let Ok(entry) = entry {
                    println!("    {:?}", entry.file_name());
                }
            }
        }
    }

    // Set the SBF_OUT_DIR environment variable to help Mollusk find the SO file
    // Try to find the correct path by going up to the project root
    if let Ok(current_dir) = env::current_dir() {
        let project_root = current_dir.parent().unwrap_or(&current_dir);
        let sbf_out_dir = project_root.join("target/deploy");
        env::set_var("SBF_OUT_DIR", sbf_out_dir.to_string_lossy().as_ref());
        println!("Set SBF_OUT_DIR to absolute path: {}", sbf_out_dir.display());
    }

    let mollusk = MolluskMt::default();
    println!("Created MolluskMt instance");

    // Create accounts
    let payer_pubkey = Pubkey::new_unique();
    let program_pubkey = Pubkey::new_unique();
    let program_data_address = get_program_data_address(&program_pubkey);
    let buffer_pubkey = Pubkey::new_unique();

    let payer_account = Account {
        lamports: 1_000_000_000, // 1 SOL
        data: vec![],
        owner: solana_system_program::id(),
        executable: false,
        rent_epoch: 0,
    };

    println!("Created test accounts");

    // Create a context
    let mut context = mollusk.with_context(HashMap::new());
    println!("Created MolluskContextMt");

    // Store payer account in the context's account store
    {
        let mut store = context.account_store.write().unwrap();
        store.insert(payer_pubkey, payer_account);
    }
    println!("Stored accounts in context");

    // Try to load the test program ELF
    println!("Attempting to load test_program_primary.so...");
    let program_load_result = std::panic::catch_unwind(|| {
        mollusk_svm::file::load_program_elf("test_program_primary")
    });

    match program_load_result {
        Ok(program_data) => {
            println!("✓ Successfully loaded program, size: {} bytes", program_data.len());

            let program_data_len = program_data.len();
            let min_rent_exempt_program_data_balance = context.mollusk.minimum_balance_for_rent_exemption(
                UpgradeableLoaderState::size_of_programdata_metadata() + program_data_len
            );
            let min_rent_exempt_program_balance = context.mollusk.minimum_balance_for_rent_exemption(
                UpgradeableLoaderState::size_of_program()
            );

            println!("Program data length: {}", program_data_len);
            println!("Rent exempt program data balance: {}", min_rent_exempt_program_data_balance);
            println!("Rent exempt program balance: {}", min_rent_exempt_program_balance);

            // Simulate the solana program deploy workflow using the upgradeable loader
            // 1. Create buffer account
            let create_buffer_ixs = loader_v3_instruction::create_buffer(
                &payer_pubkey,
                &buffer_pubkey,
                &payer_pubkey, // authority
                min_rent_exempt_program_data_balance,
                program_data_len,
            ).unwrap();

            // 2. Write program data to buffer (chunked)
            let mut instructions: Vec<solana_instruction::Instruction> = create_buffer_ixs;
            let chunk_size = 900; // Safe chunk size for transactions
            for (i, chunk) in program_data.chunks(chunk_size).enumerate() {
                let offset = i * chunk_size;
                let write_ix = loader_v3_instruction::write(
                    &buffer_pubkey,
                    &payer_pubkey, // authority
                    offset as u32,
                    chunk.to_vec(),
                );
                instructions.push(write_ix);
            }

            // 3. Deploy program from buffer
            let deploy_program_ixs = loader_v3_instruction::deploy_with_max_program_len(
                &payer_pubkey,
                &program_pubkey,
                &buffer_pubkey,
                &payer_pubkey, // upgrade authority
                min_rent_exempt_program_balance,
                program_data_len,
            ).unwrap();

            // Add deploy instructions
            instructions.extend(deploy_program_ixs);

            // Use the instructions vector directly
            let all_instructions = instructions;

            println!("Created deployment instructions");

            // Create a log collector
            let log_collector = Some(Rc::new(RefCell::new(LogCollector {
                messages: Vec::new(),
                bytes_written: 0,
                bytes_limit: Some(10000),
                limit_warning: false,
            })));

            println!("Processing instruction chain with log...");
            // Process the instruction chain with log using the context
            let (result, _transaction_context) = context.process_instruction_chain_log(
                &all_instructions,
                log_collector,
                false, // simulated
            );

            // Print some information about the result
            println!("Compute units consumed: {}", result.compute_units_consumed);
            println!("Execution time: {}", result.execution_time);
            println!("Program result: {:?}", result.program_result);

            // Check the result
            println!("Program deployment result: {:?}", result.program_result);

            // Check that accounts were handled correctly
            let store = context.account_store.read().unwrap();
            if let Some(account) = store.get(&program_pubkey) {
                println!("Program account lamports: {}", account.lamports);
                println!("Program account data length: {}", account.data.len());
                println!("Program account owner: {}", account.owner);
                println!("Program account executable: {}", account.executable);

                // Check if it's a valid UpgradeableLoaderState::Program
                match account.state() {
                    Ok(UpgradeableLoaderState::Program { programdata_address: addr }) => {
                        println!("Program account programdata_address: {}", addr);
                    }
                    Ok(state) => {
                        println!("Program account state: {:?}", state);
                    }
                    Err(e) => {
                        println!("Error parsing program account state: {:?}", e);
                    }
                }
            }

            if let Some(account) = store.get(&program_data_address) {
                println!("Program data account lamports: {}", account.lamports);
                println!("Program data account data length: {}", account.data.len());
                println!("Program data account owner: {}", account.owner);
                println!("Program data account executable: {}", account.executable);

                // Check if it's a valid UpgradeableLoaderState::ProgramData
                let elf_offset = UpgradeableLoaderState::size_of_programdata_metadata();
                if account.data.len() > elf_offset {
                    let elf_data = &account.data[elf_offset..];
                    println!("Program data ELF length: {}", elf_data.len());
                }

                match account.state() {
                    Ok(UpgradeableLoaderState::ProgramData { slot, upgrade_authority_address }) => {
                        println!("Program data account slot: {}", slot);
                        println!("Program data account upgrade_authority_address: {:?}", upgrade_authority_address);
                    }
                    Ok(state) => {
                        println!("Program data account state: {:?}", state);
                    }
                    Err(e) => {
                        println!("Error parsing program data account state: {:?}", e);
                    }
                }
            }

            if let Some(account) = store.get(&buffer_pubkey) {
                println!("Buffer account lamports: {}", account.lamports);
                println!("Buffer account data length: {}", account.data.len());
                println!("Buffer account owner: {}", account.owner);
            }

            if let Some(account) = store.get(&payer_pubkey) {
                println!("Payer account lamports: {}", account.lamports);
            }
        }
        Err(e) => {
            println!("✗ Failed to load program: {:?}", e);
            // Check if the file exists
            let file_path = Path::new("target/deploy/test_program_primary.so");
            if file_path.exists() {
                println!("✓ File exists at: {:?}", file_path);
            } else {
                println!("✗ File does not exist at: {:?}", file_path);
                // List files in target/deploy
                if let Ok(entries) = fs::read_dir("target/deploy") {
                    println!("Files in target/deploy:");
                    for entry in entries {
                        if let Ok(entry) = entry {
                            println!("  {:?}", entry.file_name());
                        }
                    }
                }
            }
        }
    }

    println!("=== Finished test_deploy_solana_program_with_sbf_out_dir ===");
}