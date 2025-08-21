//! Module for working with Solana programs.

use {
    crate::program::*,
    agave_feature_set::FeatureSet,
    solana_account::Account,
    solana_bpf_loader_program::syscalls::create_program_runtime_environment_v1,
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_loader_v3_interface::state::UpgradeableLoaderState,
    solana_loader_v4_interface::state::{LoaderV4State, LoaderV4Status},
    solana_program_runtime::{
        invoke_context::{BuiltinFunctionWithContext, InvokeContext},
        loaded_programs::{LoadProgramMetrics, ProgramCacheEntry, ProgramCacheForTxBatch},
        solana_sbpf::program::BuiltinProgram,
    },
    solana_pubkey::Pubkey,
    solana_rent::Rent,
    std::{
        cell::{RefCell, RefMut},
        collections::HashMap,
        rc::Rc,
        sync::Arc,
        sync::RwLock,
    },
};
/*
/// Loader keys, re-exported from `solana_sdk` for convenience.
pub mod loader_keys {
    pub use solana_sdk_ids::{
        bpf_loader::ID as LOADER_V2, bpf_loader_deprecated::ID as LOADER_V1,
        bpf_loader_upgradeable::ID as LOADER_V3, loader_v4::ID as LOADER_V4,
        native_loader::ID as NATIVE_LOADER,
    };
}

pub mod precompile_keys {
    use solana_pubkey::Pubkey;
    pub use solana_sdk_ids::{
        ed25519_program::ID as ED25519_PROGRAM, secp256k1_program::ID as SECP256K1_PROGRAM,
        secp256r1_program::ID as SECP256R1_PROGRAM,
    };

    pub(crate) fn is_precompile(program_id: &Pubkey) -> bool {
        matches!(
            *program_id,
            ED25519_PROGRAM | SECP256K1_PROGRAM | SECP256R1_PROGRAM
        )
    }
}*/
pub struct MtProgramCache {
    cache: Arc<RwLock<ProgramCacheForTxBatch>>,
    // This stinks, but the `ProgramCacheForTxBatch` doesn't offer a way to
    // access its entries directly. In order to make DX easier for those using
    // `MolluskContext`, we need to track entries added to the cache,
    // so we can populate the account store with program accounts.
    // This saves the developer from having to pre-load the account store with
    // all program accounts they may use, when `Mollusk` has that information
    // already.
    //
    // K: program ID, V: loader key
    entries_cache: Arc<RwLock<HashMap<Pubkey, Pubkey>>>,
    // The function registry (syscalls) to use for verifying and loading
    // program ELFs.
    pub program_runtime_environment: BuiltinProgram<InvokeContext<'static>>,
}

impl MtProgramCache {
    pub fn new(feature_set: &FeatureSet, compute_budget: &ComputeBudget) -> Self {
        let me = Self {
            cache: Arc::new(RwLock::new(ProgramCacheForTxBatch::default())),
            entries_cache: Arc::new(RwLock::new(HashMap::new())),
            program_runtime_environment: create_program_runtime_environment_v1(
                &feature_set.runtime_features(),
                &compute_budget.to_budget(),
                /* reject_deployment_of_broken_elfs */ false,
                /* debugging_features */ false,
            )
            .unwrap(),
        };
        BUILTINS.iter().for_each(|builtin| {
            let program_id = builtin.program_id;
            let entry = builtin.program_cache_entry();
            me.replenish(program_id, entry);
        });
        me
    }

    pub(crate) fn cache(&self) -> std::sync::RwLockWriteGuard<'_, ProgramCacheForTxBatch> {
        self.cache.write().unwrap()
    }

    fn replenish(&self, program_id: Pubkey, entry: Arc<ProgramCacheEntry>) {
        self.entries_cache
            .write()
            .unwrap()
            .insert(program_id, entry.account_owner());
        self.cache.write().unwrap().replenish(program_id, entry);
    }

    /// Add a builtin program to the cache.
    pub fn add_builtin(&mut self, builtin: Builtin) {
        let program_id = builtin.program_id;
        let entry = builtin.program_cache_entry();
        self.replenish(program_id, entry);
    }

    /// Add a program to the cache.
    pub fn add_program(&mut self, program_id: &Pubkey, loader_key: &Pubkey, elf: &[u8]) {
        // This might look rough, but it's actually functionally the same as
        // calling `create_program_runtime_environment_v1` on every addition.
        let environment = {
            let config = self.program_runtime_environment.get_config().clone();
            let mut loader = BuiltinProgram::new_loader(config);

            for (_key, (name, value)) in self
                .program_runtime_environment
                .get_function_registry()
                .iter()
            {
                let name = std::str::from_utf8(name).unwrap();
                loader.register_function(name, value).unwrap();
            }

            Arc::new(loader)
        };
        self.replenish(
            *program_id,
            Arc::new(
                ProgramCacheEntry::new(
                    loader_key,
                    environment,
                    0,
                    0,
                    elf,
                    elf.len(),
                    &mut LoadProgramMetrics::default(),
                )
                .unwrap(),
            ),
        );
    }

    /// Load a program from the cache.
    pub fn load_program(&self, program_id: &Pubkey) -> Option<Arc<ProgramCacheEntry>> {
        self.cache.read().unwrap().find(program_id)
    }

    // NOTE: These are only stubs. This will "just work", since Agave's SVM
    // stubs out program accounts in transaction execution already, noting that
    // the ELFs are already where they need to be: in the cache.
    pub(crate) fn get_all_keyed_program_accounts(&self) -> Vec<(Pubkey, Account)> {
        self.entries_cache
            .read()
            .unwrap()
            .iter()
            .map(|(program_id, loader_key)| match *loader_key {
                loader_keys::NATIVE_LOADER => {
                    create_keyed_account_for_builtin_program(program_id, "I'm a stub!")
                }
                loader_keys::LOADER_V1 => (*program_id, create_program_account_loader_v1(&[])),
                loader_keys::LOADER_V2 => (*program_id, create_program_account_loader_v2(&[])),
                loader_keys::LOADER_V3 => {
                    (*program_id, create_program_account_loader_v3(program_id))
                }
                loader_keys::LOADER_V4 => (*program_id, create_program_account_loader_v4(&[])),
                _ => panic!("Invalid loader key: {}", loader_key),
            })
            .collect()
    }
}
