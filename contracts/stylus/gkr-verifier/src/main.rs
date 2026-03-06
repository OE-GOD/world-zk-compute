#![cfg_attr(target_arch = "wasm32", no_main)]
#![cfg_attr(target_arch = "wasm32", no_std)]

/// Binary target required by `cargo stylus deploy` / `cargo stylus export-abi`.
///
/// When `export-abi` is enabled, this calls the generated `print_from_args()`
/// to output the Solidity ABI. On WASM, `no_main` is set so this is unused.
fn main() {
    #[cfg(feature = "export-abi")]
    gkr_verifier::print_from_args();
}
