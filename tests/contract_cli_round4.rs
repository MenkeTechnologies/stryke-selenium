//! No CLI surface to contract-test — stryke-selenium ships as a cdylib
//! loaded by stryke via dlopen, not as a `stryke-selenium-helper` binary.
//! The exports are exercised end-to-end by `t/test_selenium.stk` (FFI
//! plumbing, no WebDriver server) and the `examples/selenium_*.stk` demos
//! (live WebDriver server + real browser).
//!
//! This file is preserved (per repo convention: never delete test files)
//! and replaced with a single sanity test so `cargo test` stays green.

#[test]
fn cdylib_replacement_for_helper_binary_compiles() {}
