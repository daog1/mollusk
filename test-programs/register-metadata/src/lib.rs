#![no_std]
#![cfg_attr(target_os = "solana", feature(asm_experimental_arch))]

use solana_program_error::ProgramError;

solana_program_entrypoint::entrypoint_with_register_metadata!(process_instruction);

// With the feature enabled:
//   r1 = input region pointer (*mut u8)
//   r2 = number of accounts (u64)
//   r3 = instruction data length (u64)
//
// New SDK macro `entrypoint_with_register_metadata!` passes r1, r2, and r3 as
// parameters to the program.
fn process_instruction(
    input: *mut u8,
    num_accounts: u64,
    instruction_data_len: u64,
) -> Result<(), u64> {
    // Here, the developer can do whatever they want with this information,
    // including lazy accesses or otherwise custom deserialization.

    // For this example, we'll still use the nasty default deserializer, only
    // to test proper accesses of registry metadata.
    let (_program_id, accounts, _instruction_data) = unsafe {
        solana_program_entrypoint::deserialize(input)
    };

    // Debug: First write what we received from the macro
    let first_account = accounts
        .first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    
    // Write the metadata we received from the entrypoint macro
    let mut data = first_account.try_borrow_mut_data()?;
    data[0..8].copy_from_slice(&num_accounts.to_le_bytes());
    data[8..16].copy_from_slice(&instruction_data_len.to_le_bytes());

    Ok(())
}