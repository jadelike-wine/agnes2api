# Per-key upstream implementation plan

**Goal:** Persist one of three fixed Agnes endpoints per imported key and use it for requests.

**Architecture:** A typed optional selector lives on `KeyRecord`; its resolver is
shared by all request producers. New API imports default to International and persist an explicit selector. The existing account form and list expose the selector.

**Tech stack:** Rust, axum, serde, vanilla JavaScript, Cloudflare Workers/D1.

## 1. Storage and import

- [ ] Add failing tests in `src/core/narrow.rs` for selector round trips and malformed records;
  add an import test in `src/admin/keys.rs` with
  `{"keys":["sk-test-route"],"upstream":"china"}` expecting HTTP 200 and a persisted selector.
- [ ] Run `cargo test --lib upstream` and confirm failures against the current implementation.
- [ ] Add `KeyUpstream` with three camelCase values in `src/core/types.rs`.
  Give each variant a fixed `base_url()`, and add
  `KeyRecord::upstream_base_url(&self, fallback: &str)` with a matching returned lifetime.
- [ ] Preserve the optional selector in narrowing and `KeyView`; reject malformed present values.
- [ ] Accept the import selector before storage access, preserve duplicate metadata,
  and include it in pool write-elision comparisons and their coverage test.

## 2. Request routing

- [ ] Add a failing mixed-pool request test in `src/pool/dispatcher.rs` asserting that
  a China record produces `https://api.agnes-ai.cn/v1/chat/completions`.
- [ ] Pass the selected record to `build_request`; resolve through the shared helper.
- [ ] Apply the same resolver in `src/admin/keys.rs`, `src/admin/models.rs`,
  and `src/admin/upstream_models.rs`; add request-capture assertions for those paths.
- [ ] Run targeted tests and `cargo test --lib` (the existing GNU touch-only test
  has a known macOS incompatibility; Linux CI runs it normally).

## 3. UI and delivery

- [ ] In `admin-ui/js/sec-accounts.js`, add a labeled select before the key textarea;
  send its `upstream` value in the existing import request and display it in account metadata.
- [ ] Add all five translations in `admin-ui/js/i18n-dict.js`, including selector validation errors. Update the API documentation.
- [ ] Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --lib`, and `worker-build --profile wasm-release -- --features worker`.
- [ ] Verify the form and three-route import/reload behavior in isolated local D1;
  use mock keys only, then commit/push, deploy, and inspect the production dialog.
