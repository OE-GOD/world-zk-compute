//! C-compatible FFI bindings for zkml-verifier.
//!
//! Enables calling the verifier from C, Python (ctypes/cffi), Node.js (ffi-napi),
//! and any language that supports C ABI function calls.
//!
//! # Safety
//! All functions accepting raw pointers require the caller to guarantee:
//! - Pointers are valid and point to `len` bytes of readable memory
//! - String pointers are valid UTF-8 (for JSON functions)

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic;
use std::slice;

/// Verify a proof bundle from a JSON string.
///
/// # Safety
/// `json_ptr` must point to a valid UTF-8 C string (null-terminated).
///
/// Returns 1 if verification passes, 0 if it fails, -1 on error.
/// If `error_out` is non-null and an error occurs, a heap-allocated
/// error string is written to `*error_out`. The caller must free it
/// with `zkml_free_string`.
#[no_mangle]
pub unsafe extern "C" fn zkml_verify_json(
    json_ptr: *const c_char,
    error_out: *mut *mut c_char,
) -> i32 {
    if error_out.is_null().not() {
        unsafe { *error_out = std::ptr::null_mut() };
    }

    let result = panic::catch_unwind(|| {
        let c_str = unsafe { CStr::from_ptr(json_ptr) };
        let json = c_str.to_str().map_err(|e| format!("invalid UTF-8: {}", e))?;
        let bundle = crate::ProofBundle::from_json(json)
            .map_err(|e| format!("parse error: {}", e))?;
        let result = crate::verify(&bundle)
            .map_err(|e| format!("verify error: {}", e))?;
        Ok::<bool, String>(result.verified)
    });

    match result {
        Ok(Ok(true)) => 1,
        Ok(Ok(false)) => 0,
        Ok(Err(msg)) => {
            write_error(error_out, &msg);
            -1
        }
        Err(_) => {
            write_error(error_out, "verification panicked");
            -1
        }
    }
}

/// Verify a proof from raw byte buffers.
///
/// # Safety
/// All pointer+length pairs must be valid. `circuit_desc_json_ptr` must be
/// a null-terminated UTF-8 C string containing the DAG circuit description JSON.
///
/// Returns 1 if verification passes, 0 if it fails, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn zkml_verify_raw(
    proof_ptr: *const u8,
    proof_len: usize,
    pub_inputs_ptr: *const u8,
    pub_inputs_len: usize,
    gens_ptr: *const u8,
    gens_len: usize,
    circuit_desc_ptr: *const u8,
    circuit_desc_len: usize,
    error_out: *mut *mut c_char,
) -> i32 {
    if !error_out.is_null() {
        unsafe { *error_out = std::ptr::null_mut() };
    }

    let result = panic::catch_unwind(|| {
        let proof = unsafe { slice::from_raw_parts(proof_ptr, proof_len) };
        let pub_inputs = if pub_inputs_ptr.is_null() || pub_inputs_len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(pub_inputs_ptr, pub_inputs_len) }
        };
        let gens = unsafe { slice::from_raw_parts(gens_ptr, gens_len) };
        let circuit_desc = unsafe { slice::from_raw_parts(circuit_desc_ptr, circuit_desc_len) };

        let verified = crate::verify_dag_proof_inner(proof, pub_inputs, gens, circuit_desc);
        Ok::<bool, String>(verified)
    });

    match result {
        Ok(Ok(true)) => 1,
        Ok(Ok(false)) => 0,
        Ok(Err(msg)) => {
            write_error(error_out, &msg);
            -1
        }
        Err(_) => {
            write_error(error_out, "verification panicked");
            -1
        }
    }
}

/// Verify a proof bundle from a JSON file path.
///
/// # Safety
/// `path_ptr` must be a valid null-terminated UTF-8 C string.
///
/// Returns 1 if verification passes, 0 if it fails, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn zkml_verify_file(
    path_ptr: *const c_char,
    error_out: *mut *mut c_char,
) -> i32 {
    if !error_out.is_null() {
        unsafe { *error_out = std::ptr::null_mut() };
    }

    let result = panic::catch_unwind(|| {
        let c_str = unsafe { CStr::from_ptr(path_ptr) };
        let path = c_str.to_str().map_err(|e| format!("invalid UTF-8: {}", e))?;
        let bundle = crate::ProofBundle::from_file(path)
            .map_err(|e| format!("load error: {}", e))?;
        let result = crate::verify(&bundle)
            .map_err(|e| format!("verify error: {}", e))?;
        Ok::<bool, String>(result.verified)
    });

    match result {
        Ok(Ok(true)) => 1,
        Ok(Ok(false)) => 0,
        Ok(Err(msg)) => {
            write_error(error_out, &msg);
            -1
        }
        Err(_) => {
            write_error(error_out, "verification panicked");
            -1
        }
    }
}

/// Free a string allocated by this library.
///
/// # Safety
/// `ptr` must be a pointer returned by one of the `zkml_*` functions
/// via `error_out`, or null (which is a no-op).
#[no_mangle]
pub unsafe extern "C" fn zkml_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr) });
    }
}

/// Return the library version as a static string.
///
/// The returned pointer is valid for the lifetime of the library.
/// Do NOT call `zkml_free_string` on it.
#[no_mangle]
pub extern "C" fn zkml_version() -> *const c_char {
    static VERSION: &[u8] = b"0.1.0\0";
    VERSION.as_ptr() as *const c_char
}

/// Write an error message to the error_out pointer.
fn write_error(error_out: *mut *mut c_char, msg: &str) {
    if !error_out.is_null() {
        if let Ok(c_string) = CString::new(msg) {
            unsafe { *error_out = c_string.into_raw() };
        }
    }
}

// Helper trait to make `is_null().not()` work
trait Not {
    fn not(self) -> bool;
}
impl Not for bool {
    fn not(self) -> bool {
        !self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_version() {
        let v = zkml_version();
        let s = unsafe { CStr::from_ptr(v) }.to_str().unwrap();
        assert_eq!(s, "0.1.0");
    }

    #[test]
    fn test_verify_json_invalid() {
        let json = CString::new("{}").unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let result = unsafe { zkml_verify_json(json.as_ptr(), &mut error) };
        assert_eq!(result, -1);
        assert!(!error.is_null());
        let err_str = unsafe { CStr::from_ptr(error) }.to_str().unwrap();
        assert!(err_str.contains("error"), "unexpected error: {}", err_str);
        unsafe { zkml_free_string(error) };
    }

    #[test]
    fn test_verify_file_missing() {
        let path = CString::new("/nonexistent/path.json").unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let result = unsafe { zkml_verify_file(path.as_ptr(), &mut error) };
        assert_eq!(result, -1);
        assert!(!error.is_null());
        unsafe { zkml_free_string(error) };
    }

    #[test]
    fn test_free_null() {
        // Should be a no-op
        unsafe { zkml_free_string(std::ptr::null_mut()) };
    }
}
