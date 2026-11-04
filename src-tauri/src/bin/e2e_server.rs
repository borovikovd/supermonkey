//! The E2E server: the real core against the fake Gmail, behind the web bridge
//! and the `/__test` control API (`ub-testkit`). Until the testkit slice lands
//! it only exits, so E2E specs that need it fail while `plan.spec.ts` runs.

fn main() {
    std::process::exit(2);
}
