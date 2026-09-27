//! Write (or with `--check`, verify) the generated schema and TypeScript.
//! Run from the repository root.

use hennery_proto::codegen::{SCHEMA_PATH, TS_PATH, render_schema, render_ts};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let outputs = [(SCHEMA_PATH, render_schema()), (TS_PATH, render_ts())];
    let mut stale = false;
    for (path, content) in outputs {
        let path = Path::new(path);
        if check {
            let current = std::fs::read_to_string(path).unwrap_or_default();
            if current != content {
                eprintln!("stale generated file: {}", path.display());
                stale = true;
            }
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).expect("create output dir");
            }
            std::fs::write(path, content).expect("write generated file");
            println!("wrote {}", path.display());
        }
    }
    if stale {
        eprintln!("run `cargo run -p hennery-proto --bin gen` and commit the result");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
