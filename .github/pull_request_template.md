## Summary

<!-- What problem does this change solve, and why is this the smallest appropriate change? Link an Issue when available. -->

## Validation

<!-- List commands actually run and their results. Explain any relevant check not run. -->

- [ ] `pnpm typecheck`
- [ ] `pnpm test`
- [ ] `pnpm build`
- [ ] `pnpm check:contrast` (if UI/color/state changed)
- [ ] `pnpm licenses:check` (if dependencies changed)
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo build -p apogee-desktop --locked` and `pnpm test:e2e` (if desktop/workspace flow changed)

## Risk and review notes

<!-- Call out privacy, security, IPC, data-format, platform, package-size, startup, or memory implications. -->

- [ ] I did not include credentials, personal data, private endpoints, generated artifacts, or unrelated formatting.
- [ ] I updated documentation, tests, or migration notes where user-visible behavior changed.
- [ ] I understand my contribution is submitted under the repository's MIT license.
