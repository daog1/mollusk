//! Module for customizing the VM Mollusk operates on.

use {
    mollusk_svm_result::InstructionResult, solana_account::Account,
    solana_instruction::Instruction, solana_pubkey::Pubkey,
};

/// The SVM trait defines the interface for a Solana Virtual Machine (SVM) that
/// can process instructions.
///
/// Developers can extend Mollusk to apply state transition functions across
/// custom SVMs by implementing this trait.
pub trait SVM {
    // TODO: The correct thing to do is to couple program JIT caching with the
    // SVM implementation, so custom SVMs can move away from Agave's
    // program-runtime if they see fit.
    //
    // Ideally, this whole trait should allow `Mollusk` to be generic over
    // an SVM, where AgaveSVM implements using `solana-program-runtime`.
    // This, `solana-program-runtime` would no longer be a direct dependency
    // of `Mollusk`, but rather a dependency of the SVM implementation.
    // fn add_program_with_elf_and_loader(
    //     &mut self,
    //     program_id: &Pubkey,
    //     elf: &[u8],
    //     loader_key: &Pubkey,
    // );

    fn process_instruction(
        &self,
        instruction: &Instruction,
        accounts: &[(Pubkey, Account)],
    ) -> InstructionResult;
}
