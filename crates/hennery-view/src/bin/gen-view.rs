//! Write (or with `--check`, verify) the view's generated schema and
//! TypeScript. Run from the repository root. Not named `gen`:
//! `hennery-proto`'s generator is, and two binaries of one name overwrite
//! each other in `target/` when the workspace is built. Its exit statuses
//! are `hennery_view::codegen::run`'s.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    ExitCode::from(hennery_view::codegen::run(Path::new("."), check))
}
