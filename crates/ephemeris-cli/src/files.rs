//! Reading and writing the JSON contracts, with errors that name the file.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// `what` names the document in errors, e.g. "feed".
pub fn read_json<T: DeserializeOwned>(path: &Path, what: &str) -> Result<T, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .map_err(|e| format!("{} is not a valid {what}: {e}", path.display()))
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("cannot write {}: {e}", path.display());
    let file = File::create(path).map_err(|e| fail(&e))?;
    let mut w = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut w, value).map_err(|e| fail(&e))?;
    w.write_all(b"\n")
        .and_then(|()| w.flush())
        .map_err(|e| fail(&e))
}
