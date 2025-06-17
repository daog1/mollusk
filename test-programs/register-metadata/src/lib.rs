
use solana_program_error::ProgramError;

solana_program_entrypoint::entrypoint_with_input_data_offset!(process_instruction);

// With the feature enabled:
//   r1 = input region pointer (*mut u8)
//   r2 = offset to instruction data within the input region
//     [length][data...]
//
// New SDK macro `entrypoint_with_input_data_offset!` passes r1 and r2 as
// parameters to the program.
fn process_instruction(
    input: *mut u8,
    instruction_data_offset: u64,
) -> Result<(), u64> {
    // The developer receives the instruction data offset as a VM virtual
    // address. This allows programs to skip account deserialization and jump
    // directly to the instruction data when needed.

    // For this test, we'll demonstrate that the offset is being passed
    // correctly by using it to write the input data to the first account.
    let (_program_id, accounts, _instruction_data) = unsafe {
        solana_program_entrypoint::deserialize(input)
    };

    // Write the instruction data to the first account's data using the offset.
    let mut data = accounts
        .first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?
        .try_borrow_mut_data()?;
    
    // The instruction data offset points to the length of the instruction
    // data (as a u64), followed immediately by the data itself.
    let input_data = unsafe {
        let len_ptr = instruction_data_offset as *const u64;
        let len = core::ptr::read(len_ptr);
        
        let bytes_ptr = (instruction_data_offset + 8) as *const u8;
        let bytes = core::slice::from_raw_parts(bytes_ptr, len as usize);

        bytes
    };
    
    data[..].copy_from_slice(&input_data);

    Ok(())
}