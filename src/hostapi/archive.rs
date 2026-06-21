//! The `archive` module (PRD §7): extract zip and tar.gz without relying
//! on `tar`/`unzip` existing on the target — bootstrapping must not
//! depend on them.

use std::path::Path;

use wscript::Module;

fn extract_zip(archive: &str, dest: &str) -> Result<i64, String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("cannot open {archive}: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    // extract() guards against path traversal internally.
    zip.extract(Path::new(dest)).map_err(|e| e.to_string())?;
    Ok(zip.len() as i64)
}

fn extract_tar_gz(archive: &str, dest: &str) -> Result<i64, String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("cannot open {archive}: {e}"))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut count = 0i64;
    // unpack_in guards each entry against escaping dest.
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        entry.unpack_in(dest).map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}

pub fn module() -> Module {
    let mut m = Module::new("archive");
    m.doc("Extract archives (no external tar/unzip needed)");

    m.doc_next("Extract a .zip into a directory; returns the entry count");
    m.fn_(
        "extract_zip",
        |archive: &str, dest: &str| -> Result<i64, String> { extract_zip(archive, dest) },
    );
    m.doc_next("Extract a .tar.gz into a directory; returns the entry count");
    m.fn_(
        "extract_tar_gz",
        |archive: &str, dest: &str| -> Result<i64, String> { extract_tar_gz(archive, dest) },
    );
    m.doc_next("Extract by file extension (.zip, .tar.gz, .tgz)");
    m.fn_(
        "extract",
        |archive: &str, dest: &str| -> Result<i64, String> {
            if archive.ends_with(".zip") {
                extract_zip(archive, dest)
            } else if archive.ends_with(".tar.gz") || archive.ends_with(".tgz") {
                extract_tar_gz(archive, dest)
            } else {
                Err(format!("unsupported archive type: {archive}"))
            }
        },
    );
    m
}
