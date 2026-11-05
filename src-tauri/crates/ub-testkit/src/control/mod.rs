//! The `/__test` control API that `e2e/control.ts` calls (ARCHITECTURE.md,
//! section 2: Test control), and the web around the app on the same origin.

mod accounts;
mod faults;
mod gmail;
mod lifecycle;
mod routes;
mod web;

pub(crate) use routes::router;
