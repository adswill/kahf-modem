//! Loads the HackRF and RTL-SDR libraries when they are first used, so the program
//! starts without them. The open functions check for the library and say what to
//! install if it is missing.
use libloading::Library;
use std::sync::OnceLock;

const HACKRF_NAMES: &[&str] = &[
    "libhackrf.dylib",
    "/opt/homebrew/lib/libhackrf.dylib",
    "/usr/local/lib/libhackrf.dylib",
    "libhackrf.so.0",
    "libhackrf.so",
    "hackrf.dll",
];
const RTLSDR_NAMES: &[&str] = &[
    "librtlsdr.dylib",
    "/opt/homebrew/lib/librtlsdr.dylib",
    "/usr/local/lib/librtlsdr.dylib",
    "librtlsdr.so.2",
    "librtlsdr.so",
    "rtlsdr.dll",
];

fn load(names: &[&str]) -> Option<Library> {
    for name in names {
        // SAFETY: only the vendor library names listed above are loaded.
        if let Ok(lib) = unsafe { Library::new(name) } {
            return Some(lib);
        }
    }
    None
}

/// The HackRF library, if it is installed.
pub(crate) fn hackrf() -> Option<&'static Library> {
    static LIB: OnceLock<Option<Library>> = OnceLock::new();
    LIB.get_or_init(|| load(HACKRF_NAMES)).as_ref()
}

/// The RTL-SDR library, if it is installed.
pub(crate) fn rtlsdr() -> Option<&'static Library> {
    static LIB: OnceLock<Option<Library>> = OnceLock::new();
    LIB.get_or_init(|| load(RTLSDR_NAMES)).as_ref()
}

/// One function from a loaded library.
pub(crate) fn symbol<F: Copy>(lib: &'static Library, name: &str) -> Option<F> {
    let c = format!("{name}\0");
    // SAFETY: the caller names the function and its signature; both come from the vendor headers.
    unsafe { lib.get::<F>(c.as_bytes()).ok().map(|s| *s) }
}

/// Declares foreign functions that are looked up in the library when called.
/// A call made without the library returns zeroed values; the open functions
/// check for the library before any such call.
macro_rules! foreign_fns {
    ($lib:ident; $( fn $name:ident ( $( $arg:ident : $ty:ty ),* $(,)? ) $( -> $ret:ty )? ; )*) => {
        $(
            /// Calls the library function of the same name.
            #[allow(clippy::missing_safety_doc)]
            pub unsafe fn $name( $( $arg: $ty ),* ) $( -> $ret )? {
                type F = unsafe extern "C" fn( $( $ty ),* ) $( -> $ret )?;
                let found = $crate::dl::$lib().and_then(|l| $crate::dl::symbol::<F>(l, stringify!($name)));
                match found {
                    Some(f) => unsafe { f( $( $arg ),* ) },
                    None => unsafe { std::mem::zeroed() },
                }
            }
        )*
    };
}
pub(crate) use foreign_fns;
