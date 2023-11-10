use crate::config::ConfigInternal;
use crate::ln_dlc::node::Node;
use crate::storage::TenTenOneNodeStorage;
use ln_dlc_node::seed::Bip39Seed;
use state::Storage;
use std::sync::Arc;

// FIXME(holzeis): mutability is only required for tests, but somehow annotating them with
// #[cfg(test)] and #[cfg(not(test))] did not work. The tests are always compiled with
// #[cfg(not(test))]

/// For testing we need the state to be mutable as otherwise we can't start another app after
/// stopping the first one. Note, running two apps at the same time will not work as the states
/// below are static and will be used for both apps.
/// TODO(holzeis): Check if there is a way to bind the state to the lifetime of the app (node).

static mut CONFIG: TenTenOneState<ConfigInternal> = TenTenOneState::new();
static mut NODE: TenTenOneState<Arc<Node>> = TenTenOneState::new();
static mut SEED: TenTenOneState<Bip39Seed> = TenTenOneState::new();
static mut STORAGE: TenTenOneState<TenTenOneNodeStorage> = TenTenOneState::new();

pub struct TenTenOneState<T: Send + Sync + Clone> {
    inner: Storage<T>,
}

impl<T: Send + Sync + Clone> TenTenOneState<T> {
    pub const fn new() -> TenTenOneState<T> {
        Self {
            inner: Storage::new(),
        }
    }

    fn set(&mut self, state: T) {
        match self.inner.try_get_mut() {
            Some(inner_state) => *inner_state = state,
            None => {
                self.inner = Storage::from(state);
            }
        }
    }

    fn get(&self) -> T {
        self.inner.get().clone()
    }

    fn try_get(&self) -> Option<T> {
        self.inner.try_get().cloned()
    }
}

pub fn set_config(config: ConfigInternal) {
    unsafe {
        CONFIG.set(config);
    }
}

pub fn get_config() -> ConfigInternal {
    unsafe { CONFIG.get() }
}

pub fn set_node(node: Arc<Node>) {
    unsafe {
        NODE.set(node);
    }
}

pub fn get_node() -> Arc<Node> {
    unsafe { NODE.get() }
}

pub fn try_get_node() -> Option<Arc<Node>> {
    unsafe { NODE.try_get() }
}

pub fn set_seed(seed: Bip39Seed) {
    unsafe {
        SEED.set(seed);
    }
}

pub fn get_seed() -> Bip39Seed {
    unsafe { SEED.get() }
}

pub fn try_get_seed() -> Option<Bip39Seed> {
    unsafe { SEED.try_get() }
}

pub fn set_storage(storage: TenTenOneNodeStorage) {
    unsafe {
        STORAGE.set(storage);
    }
}

pub fn get_storage() -> TenTenOneNodeStorage {
    unsafe { STORAGE.get() }
}

pub fn try_get_storage() -> Option<TenTenOneNodeStorage> {
    unsafe { STORAGE.try_get() }
}
