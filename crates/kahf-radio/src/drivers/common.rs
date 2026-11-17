//! Shared helpers for drivers: loading a vendor library at run time.

use libloading::Library;

/// Loads the first library in `names` that the system has. The error lists what was tried.
pub fn load_library(names: &[&str]) -> Result<Library, String> {
    let mut tried = Vec::new();
    for name in names {
        // SAFETY: loading a shared library runs its initialisers; only vendor libraries are named here.
        match unsafe { Library::new(name) } {
            Ok(lib) => return Ok(lib),
            Err(e) => tried.push(format!("{name} ({e})")),
        }
    }
    Err(format!("library not found; tried {}", tried.join(", ")))
}
