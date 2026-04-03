# When a task is completed
- Run the smallest relevant verification first; preferred defaults are `cargo fmt --check --all`, `cargo check --workspace --all-targets`, and targeted or full `cargo test` depending on scope.
- For architecture/boundary changes, also run `cargo run -p xtask -- check-boundaries`.
- If runtime packaging behavior is affected, verify with `cargo run -p xtask -- package-plugins` and, if appropriate, `cargo run -p revy-server`.
- Mention any known baseline gaps from `docs/contributors/known-issues.md` if they affect validation.