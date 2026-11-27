//! C-compatible FFI bindings for zkml-verifier.
//!
//! Enables calling the verifier from C, Python (ctypes/cffi), Node.js (ffi-napi),
//! and any language that supports C ABI function calls.
//!
//! # Safety
//! All functions accepting raw pointers require the caller to guarantee:
//! - Non-null pointers are valid and point to `len` bytes of readable memory
//! - String pointers are null-terminated and valid UTF-8 (for JSON functions)
//!
//! A null `json_ptr` or `path_ptr`, and a null byte pointer with a non-zero
//! length, are rejected with return value -1. `error_out` may be null. The
//! public-input pointer and length of `zkml_verify_raw` are never read: the
//! public-input bytes are not used by the verification.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic;
use std::slice;

/// Verify a proof bundle from a JSON string.
///
/// # Safety
/// `json_ptr` must point to a valid UTF-8 C string (null-terminated).
///
/// Returns 1 if the proof verifies, -1 otherwise (rejected proof or error;
/// see `error_out`).
/// If `error_out` is non-null and an error occurs, a heap-allocated
/// error string is written to `*error_out`. The caller must free it
/// with `zkml_free_string`.
#[no_mangle]
pub unsafe extern "C" fn zkml_verify_json(
    json_ptr: *const c_char,
    error_out: *mut *mut c_char,
) -> i32 {
    if !error_out.is_null() {
        unsafe { *error_out = std::ptr::null_mut() };
    }

    if json_ptr.is_null() {
        write_error(error_out, "json_ptr is null");
        return -1;
    }

    let result = panic::catch_unwind(|| {
        let c_str = unsafe { CStr::from_ptr(json_ptr) };
        let json = c_str
            .to_str()
            .map_err(|e| format!("invalid UTF-8: {}", e))?;
        let bundle =
            crate::ProofBundle::from_json(json).map_err(|e| format!("parse error: {}", e))?;
        let result = crate::verify(&bundle).map_err(|e| format!("verify error: {}", e))?;
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
/// All pointer+length pairs must be valid. `circuit_desc_ptr` points to the
/// circuit description in the flat 32-byte-word encoding (the same bytes the
/// on-chain verifier takes), not to JSON. A null pointer is accepted only
/// with length 0.
///
/// The public inputs are not used: the verifier reads the inputs embedded in
/// the proof. The parameters are kept for ABI compatibility.
///
/// The buffers are decoded with the same checked decoders as in the other
/// entry points, so the same limits apply: an array of more than 100,000
/// elements in the proof or in the generators is an error.
///
/// Returns 1 if the proof verifies, -1 otherwise (rejected proof or error;
/// see `error_out`).
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

    // Unused by the verifier; see the function documentation.
    let _ = (pub_inputs_ptr, pub_inputs_len);

    let (proof, gens, circuit_desc) = unsafe {
        match (
            slice_or_empty(proof_ptr, proof_len),
            slice_or_empty(gens_ptr, gens_len),
            slice_or_empty(circuit_desc_ptr, circuit_desc_len),
        ) {
            (Some(p), Some(g), Some(c)) => (p, g, c),
            _ => {
                write_error(error_out, "null pointer with non-zero length");
                return -1;
            }
        }
    };

    let result = panic::catch_unwind(|| {
        let result = crate::verify_raw(proof, gens, circuit_desc)
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

/// Verify a proof bundle from a JSON file path.
///
/// # Safety
/// `path_ptr` must be a valid null-terminated UTF-8 C string.
///
/// Returns 1 if the proof verifies, -1 otherwise (rejected proof or error;
/// see `error_out`).
#[no_mangle]
pub unsafe extern "C" fn zkml_verify_file(
    path_ptr: *const c_char,
    error_out: *mut *mut c_char,
) -> i32 {
    if !error_out.is_null() {
        unsafe { *error_out = std::ptr::null_mut() };
    }

    if path_ptr.is_null() {
        write_error(error_out, "path_ptr is null");
        return -1;
    }

    let result = panic::catch_unwind(|| {
        let c_str = unsafe { CStr::from_ptr(path_ptr) };
        let path = c_str
            .to_str()
            .map_err(|e| format!("invalid UTF-8: {}", e))?;
        let bundle =
            crate::ProofBundle::from_file(path).map_err(|e| format!("load error: {}", e))?;
        let result = crate::verify(&bundle).map_err(|e| format!("verify error: {}", e))?;
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

/// Build a byte slice from a pointer+length pair. A null pointer is only
/// valid together with length 0 (and yields an empty slice).
///
/// # Safety
/// A non-null `ptr` must point to `len` readable bytes.
unsafe fn slice_or_empty<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        Some(&[])
    } else if ptr.is_null() {
        None
    } else {
        Some(unsafe { slice::from_raw_parts(ptr, len) })
    }
}

/// Write an error message to the error_out pointer.
fn write_error(error_out: *mut *mut c_char, msg: &str) {
    if !error_out.is_null() {
        if let Ok(c_string) = CString::new(msg) {
            unsafe { *error_out = c_string.into_raw() };
        }
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
    fn test_verify_json_null_pointer() {
        let mut error: *mut c_char = std::ptr::null_mut();
        let result = unsafe { zkml_verify_json(std::ptr::null(), &mut error) };
        assert_eq!(result, -1);
        assert!(!error.is_null());
        unsafe { zkml_free_string(error) };
        // A null error_out is allowed too.
        let result = unsafe { zkml_verify_json(std::ptr::null(), std::ptr::null_mut()) };
        assert_eq!(result, -1);
    }

    #[test]
    fn test_verify_file_null_pointer() {
        let mut error: *mut c_char = std::ptr::null_mut();
        let result = unsafe { zkml_verify_file(std::ptr::null(), &mut error) };
        assert_eq!(result, -1);
        assert!(!error.is_null());
        unsafe { zkml_free_string(error) };
    }

    #[test]
    fn test_verify_json_malformed_circuit_description() {
        // A well-formed bundle whose circuit description lacks every field
        // must come back as an error string, not as a panic.
        let proof = format!("0x{}{}", hex::encode(b"REM1"), "00".repeat(64));
        let json = CString::new(format!(
            r#"{{"proof_hex":"{proof}","gens_hex":"0x","dag_circuit_description":{{}}}}"#
        ))
        .unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let result = unsafe { zkml_verify_json(json.as_ptr(), &mut error) };
        assert_eq!(result, -1);
        let err_str = unsafe { CStr::from_ptr(error) }.to_str().unwrap();
        assert!(
            err_str.contains("dag_circuit_description.numComputeLayers"),
            "unexpected error: {}",
            err_str
        );
        unsafe { zkml_free_string(error) };
    }

    #[test]
    fn test_verify_raw_null_and_empty() {
        let mut error: *mut c_char = std::ptr::null_mut();
        // Null pointer with a non-zero length is rejected without being read.
        let result = unsafe {
            zkml_verify_raw(
                std::ptr::null(),
                64,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                &mut error,
            )
        };
        assert_eq!(result, -1);
        let err_str = unsafe { CStr::from_ptr(error) }.to_str().unwrap();
        assert!(err_str.contains("null pointer"), "unexpected: {}", err_str);
        unsafe { zkml_free_string(error) };

        // All-null with zero lengths is an (invalid) empty proof: an error, not a crash.
        let mut error: *mut c_char = std::ptr::null_mut();
        let result = unsafe {
            zkml_verify_raw(
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                &mut error,
            )
        };
        assert_eq!(result, -1);
        let err_str = unsafe { CStr::from_ptr(error) }.to_str().unwrap();
        assert!(err_str.contains("too short"), "unexpected: {}", err_str);
        unsafe { zkml_free_string(error) };
    }

    #[test]
    fn test_verify_raw_hostile_description_length() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contracts/test/fixtures/phase1a_dag_fixture.json"
        );
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture_path).unwrap()).unwrap();
        let proof = hex::decode(&fixture["proof_hex"].as_str().unwrap()[2..]).unwrap();
        let gens = hex::decode(&fixture["gens_hex"].as_str().unwrap()[2..]).unwrap();
        // The proof and the generators are the real ones, so decoding gets as
        // far as the description. Its first array-length word (layerTypes,
        // after the two layer counts) is set to 2^47.
        let mut desc =
            crate::test_utils::encode_circuit_desc_from_json(&fixture["dag_circuit_description"]);
        desc[64..96].fill(0);
        desc[64 + 26] = 0x80;
        let mut error: *mut c_char = std::ptr::null_mut();
        let result = unsafe {
            zkml_verify_raw(
                proof.as_ptr(),
                proof.len(),
                std::ptr::null(),
                0,
                gens.as_ptr(),
                gens.len(),
                desc.as_ptr(),
                desc.len(),
                &mut error,
            )
        };
        assert_eq!(result, -1);
        assert!(!error.is_null());
        let err_str = unsafe { CStr::from_ptr(error) }.to_str().unwrap();
        assert!(
            err_str.contains("array length 140737488355328"),
            "unexpected error: {}",
            err_str
        );
        unsafe { zkml_free_string(error) };
    }

    #[test]
    fn test_free_null() {
        // Should be a no-op
        unsafe { zkml_free_string(std::ptr::null_mut()) };
    }
}
