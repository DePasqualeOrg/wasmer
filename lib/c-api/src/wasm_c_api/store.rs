use super::engine::wasm_engine_t;
use std::cell::UnsafeCell;
use std::rc::Rc;
use wasmer_api::{AsStoreMut, AsStoreRef, Store, StoreMut, StoreRef as BaseStoreRef};

#[derive(Clone)]
pub struct StoreRef {
    inner: Rc<UnsafeCell<Store>>,
}

impl StoreRef {
    /// Returns true if two StoreRefs point to the same underlying store.
    pub(crate) fn ptr_eq(&self, other: &StoreRef) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }

    pub unsafe fn store(&self) -> BaseStoreRef<'_> {
        unsafe { (*self.inner.get()).as_store_ref() }
    }

    pub unsafe fn store_mut(&mut self) -> StoreMut<'_> {
        unsafe { (*self.inner.get()).as_store_mut() }
    }

    /// Temporarily take ownership of the underlying Store, pass it to `f`,
    /// and put the returned Store back.
    ///
    /// This uses `ptr::read`/`ptr::write` to move the Store out and back.
    /// Between the two operations, the `UnsafeCell` holds moved-from memory.
    /// If `f` panics, the store cannot be restored, so the process aborts.
    ///
    /// # Safety
    /// The caller must ensure no other code accesses the store during `f`.
    pub(crate) unsafe fn with_owned_store<R>(&self, f: impl FnOnce(Store) -> (Store, R)) -> R {
        let store = unsafe { std::ptr::read(self.inner.get()) };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(store)));
        match result {
            Ok((store, r)) => {
                unsafe { std::ptr::write(self.inner.get(), store) };
                r
            }
            Err(_) => {
                // The store was consumed by f and can't be recovered.
                // Abort to avoid use-after-free from the moved-from cell.
                std::process::abort();
            }
        }
    }
}

/// Opaque type representing a WebAssembly store.
#[allow(non_camel_case_types)]
pub struct wasm_store_t {
    pub(crate) inner: StoreRef,
}

/// Creates a new WebAssembly store given a specific [engine][super::engine].
///
/// # Example
///
/// See the module's documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasm_store_new(
    engine: Option<&wasm_engine_t>,
) -> Option<Box<wasm_store_t>> {
    let engine = engine?;
    let store = Store::new(engine.inner.clone());

    Some(Box::new(wasm_store_t {
        inner: StoreRef {
            inner: Rc::new(UnsafeCell::new(store)),
        },
    }))
}

/// Deletes a WebAssembly store.
///
/// # Example
///
/// See the module's documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasm_store_delete(_store: Option<Box<wasm_store_t>>) {}
