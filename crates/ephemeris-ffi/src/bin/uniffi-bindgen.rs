//! The UniFFI bindings generator, built from the same UniFFI version as the
//! library so the generated Swift matches its ABI. Only
//! scripts/build-ios.sh runs it (`--features bindgen`).

fn main() {
    uniffi::uniffi_bindgen_main()
}
