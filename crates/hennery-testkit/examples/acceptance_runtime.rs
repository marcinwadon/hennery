//! Install the embedded candidate in a new, explicit scratch directory.
use anyhow::{Context, Result, ensure};
use hennery_host::runtime::{
    download::Sources,
    install::{self, Layout, Selection},
};
use std::{collections::BTreeSet, path::PathBuf};

#[tokio::main]
async fn main() -> Result<()> {
    let destination = PathBuf::from(std::env::args_os().nth(1).context("supply a new scratch directory")?);
    ensure!(
        destination.is_absolute() && !destination.exists(),
        "destination must be new and absolute"
    );
    let selection = Selection::pinned(&BTreeSet::new())?;
    let installed = install::install(
        &Layout::new(&destination)?,
        &selection,
        &Sources::new(None, None)?,
        &|line| eprintln!("{line}"),
    )
    .await?;
    let set = installed.set();
    println!(
        "{}",
        serde_json::json!({"set": set.path, "node": set.node, "record": set.record})
    );
    Ok(())
}
