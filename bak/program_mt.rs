pub struct ProgramCache {
    cache: Rc<RefCell<ProgramCacheForTxBatch>>,
    // This stinks, but the `ProgramCacheForTxBatch` doesn't offer a way to
    // access its entries directly. In order to make DX easier for those using
    // `MolluskContext`, we need to track entries added to the cache,
    // so we can populate the account store with program accounts.
    // This saves the developer from having to pre-load the account store with
    // all program accounts they may use, when `Mollusk` has that information
    // already.
    //
    // K: program ID, V: loader key
    entries_cache: Rc<RefCell<HashMap<Pubkey, Pubkey>>>,
    // The function registry (syscalls) to use for verifying and loading
    // program ELFs.
    pub program_runtime_environment: BuiltinProgram<InvokeContext<'static>>,
}
