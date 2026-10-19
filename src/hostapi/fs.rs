//! The `fs` module: file IO for scripts (PRD §7). Richer than wscript-std's
//! `fs` — metadata, glob, temp files/dirs, symlinks — so Config Weave
//! registers this module instead of the stdlib one.

use std::collections::HashMap;
use std::path::Path;

use wscript::Module;
use wscript_std::DynValue;

fn err(e: std::io::Error) -> String {
    e.to_string()
}

pub fn module() -> Module {
    let mut m = Module::new("fs");
    m.doc("File IO (capability: filesystem access)");

    m.doc_next("Read a file as text");
    m.fn_("read", |path: &str| -> Result<String, String> {
        std::fs::read_to_string(path).map_err(err)
    });
    m.doc_next("Read a file as bytes");
    m.fn_("read_bytes", |path: &str| -> Result<Vec<i64>, String> {
        std::fs::read(path)
            .map(|b| b.into_iter().map(|x| x as i64).collect())
            .map_err(err)
    });
    m.doc_next("Write text to a file, replacing its contents");
    m.fn_("write", |path: &str, content: &str| -> Result<(), String> {
        std::fs::write(path, content).map_err(err)
    });
    m.doc_next("Write bytes to a file, replacing its contents");
    m.fn_(
        "write_bytes",
        |path: &str, content: Vec<i64>| -> Result<(), String> {
            let bytes: Vec<u8> = content.iter().map(|b| *b as u8).collect();
            std::fs::write(path, bytes).map_err(err)
        },
    );
    m.doc_next(
        "Write text to a file, replacing its contents, with options {follow_symlinks: bool}. \
         follow_symlinks false refuses a final symlink instead of writing through it",
    );
    m.fn_(
        "write_with",
        |path: &str, content: &str, opts: DynValue| -> Result<(), String> {
            write_with(path, content.as_bytes(), &opts)
        },
    );
    m.doc_next(
        "Write bytes to a file, replacing its contents, with options {follow_symlinks: bool}. \
         follow_symlinks false refuses a final symlink instead of writing through it",
    );
    m.fn_(
        "write_bytes_with",
        |path: &str, content: Vec<i64>, opts: DynValue| -> Result<(), String> {
            let bytes: Vec<u8> = content.iter().map(|b| *b as u8).collect();
            write_with(path, &bytes, &opts)
        },
    );
    m.doc_next("Append text to a file, creating it if absent");
    m.fn_(
        "append",
        |path: &str, content: &str| -> Result<(), String> {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| f.write_all(content.as_bytes()))
                .map_err(err)
        },
    );
    m.doc_next("Copy a file");
    m.fn_("copy", |from: &str, to: &str| -> Result<(), String> {
        std::fs::copy(from, to).map(|_| ()).map_err(err)
    });
    m.doc_next("Move (rename) a file or directory");
    m.fn_("move", |from: &str, to: &str| -> Result<(), String> {
        std::fs::rename(from, to).map_err(err)
    });
    m.doc_next("Delete a file or symlink");
    m.fn_("delete", |path: &str| -> Result<(), String> {
        std::fs::remove_file(path).map_err(err)
    });
    m.doc_next("Delete a directory and everything in it");
    m.fn_("delete_dir", |path: &str| -> Result<(), String> {
        std::fs::remove_dir_all(path).map_err(err)
    });
    m.doc_next("Create a directory, including missing parents");
    m.fn_("mkdir", |path: &str| -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(err)
    });
    m.doc_next(
        "Whether a path exists. Follows symlinks: a dangling symlink answers false (see lexists)",
    );
    m.fn_("exists", |path: &str| Path::new(path).exists());
    m.doc_next(
        "Whether a path exists without following a final symlink: a dangling symlink answers true",
    );
    m.fn_("lexists", |path: &str| lexists(path));
    m.doc_next("Whether a path is a file");
    m.fn_("is_file", |path: &str| Path::new(path).is_file());
    m.doc_next("Whether a path is a directory");
    m.fn_("is_dir", |path: &str| Path::new(path).is_dir());
    m.doc_next("List entries of a directory (sorted names)");
    m.fn_("list_dir", |path: &str| -> Result<Vec<String>, String> {
        let mut entries: Vec<String> = std::fs::read_dir(path)
            .map_err(err)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        Ok(entries)
    });
    m.doc_next(
        "File metadata as a map: size, modified (unix secs), readonly, is_file, is_dir, \
         is_symlink, mode (unix permission bits, st_mode & 0o7777; 0 elsewhere), uid and gid \
         (unix owner ids; null elsewhere). Does not follow a final symlink: for a link it \
         describes the link itself",
    );
    m.fn_("metadata", |path: &str| metadata(path));
    m.doc_next("Expand a glob pattern to matching paths (sorted)");
    m.fn_("glob", |pattern: &str| -> Result<Vec<String>, String> {
        let paths = glob::glob(pattern).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for p in paths {
            out.push(p.map_err(|e| e.to_string())?.display().to_string());
        }
        out.sort();
        Ok(out)
    });
    m.doc_next("Create a fresh temporary file and return its path");
    m.fn_("temp_file", || -> Result<String, String> {
        let f = tempfile::NamedTempFile::new().map_err(err)?;
        let (_, path) = f.keep().map_err(|e| e.to_string())?;
        Ok(path.display().to_string())
    });
    m.doc_next("Create a fresh temporary directory and return its path");
    m.fn_("temp_dir", || -> Result<String, String> {
        let d = tempfile::TempDir::new().map_err(err)?;
        Ok(d.keep().display().to_string())
    });
    m.doc_next("Create a symlink at `link` pointing to `target`");
    m.fn_(
        "symlink",
        |target: &str, link: &str| -> Result<(), String> {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, link).map_err(err)
            }
            #[cfg(windows)]
            {
                if Path::new(target).is_dir() {
                    std::os::windows::fs::symlink_dir(target, link).map_err(err)
                } else {
                    std::os::windows::fs::symlink_file(target, link).map_err(err)
                }
            }
        },
    );
    m.doc_next("Read the target of a symlink");
    m.fn_("read_link", |path: &str| -> Result<String, String> {
        std::fs::read_link(path)
            .map(|p| p.display().to_string())
            .map_err(err)
    });
    m.doc_next(
        "Change a path's owner and group (Unix only). Each is a user/group name, a numeric id, \
         or Value::Null to leave it unchanged. Does not follow a final symlink",
    );
    m.fn_(
        "chown",
        |path: &str, owner: DynValue, group: DynValue| -> Result<(), String> {
            chown(path, &owner, &group)
        },
    );
    m.doc_next(
        "Change a path's permission bits (Unix only). mode is an int (448 for 0o700) or a \
         string of octal digits (\"0700\"), at most 0o7777. Refuses a final symlink rather \
         than following it",
    );
    m.fn_(
        "chmod",
        |path: &str, mode: DynValue| -> Result<(), String> { chmod(path, &mode) },
    );
    m
}

fn lexists(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn parse_write_opts(opts: &DynValue) -> Result<bool, String> {
    let mut follow = true;
    match opts {
        DynValue::Null => {}
        DynValue::Map(m) => {
            for (k, v) in m {
                match (k.as_str(), v) {
                    ("follow_symlinks", DynValue::Bool(b)) => follow = *b,
                    ("follow_symlinks", other) => {
                        return Err(format!("follow_symlinks must be a bool, got {other:?}"));
                    }
                    (other, _) => {
                        return Err(format!(
                            "unknown write option '{other}' (expected follow_symlinks)"
                        ));
                    }
                }
            }
        }
        other => {
            return Err(format!(
                "write options must be a map or null, got {other:?}"
            ));
        }
    }
    Ok(follow)
}

/// With `follow_symlinks: false` a root-run write into a user-owned tree
/// cannot be redirected by a planted link, as with `chown`.
fn write_with(path: &str, content: &[u8], opts: &DynValue) -> Result<(), String> {
    if parse_write_opts(opts)? {
        return std::fs::write(path, content).map_err(err);
    }
    write_no_follow(path, content)
}

fn refuse_symlink_write(path: &str) -> String {
    format!("cannot write {path}: it is a symlink and follow_symlinks is false")
}

/// O_NOFOLLOW makes the open itself fail on a final symlink, so there is
/// no window between a check and the write.
#[cfg(unix)]
fn write_no_follow(path: &str, content: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path);
    match file {
        Ok(mut f) => f.write_all(content).map_err(err),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => Err(refuse_symlink_write(path)),
        Err(e) => Err(err(e)),
    }
}

/// Check-then-open, not atomic: a link swapped in between the check and
/// the open is followed. Any reparse point (symlink or junction) counts.
#[cfg(windows)]
fn write_no_follow(path: &str, content: &[u8]) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && (meta.file_type().is_symlink()
            || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    {
        return Err(refuse_symlink_write(path));
    }
    std::fs::write(path, content).map_err(err)
}

/// `mode` keeps its `0` off Unix so existing `as_int()` readers still
/// work; `uid`/`gid` are `Null` there because `0` would claim root.
fn metadata(path: &str) -> Result<DynValue, String> {
    let meta = std::fs::symlink_metadata(path).map_err(err)?;
    let mut map = HashMap::new();
    map.insert("size".into(), DynValue::Int(meta.len() as i64));
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    map.insert("modified".into(), DynValue::Int(modified));
    map.insert(
        "readonly".into(),
        DynValue::Bool(meta.permissions().readonly()),
    );
    map.insert("is_file".into(), DynValue::Bool(meta.is_file()));
    map.insert("is_dir".into(), DynValue::Bool(meta.is_dir()));
    map.insert(
        "is_symlink".into(),
        DynValue::Bool(meta.file_type().is_symlink()),
    );
    #[cfg(unix)]
    let (mode, uid, gid) = {
        use std::os::unix::fs::MetadataExt;
        (
            DynValue::Int((meta.mode() & 0o7777) as i64),
            DynValue::Int(meta.uid() as i64),
            DynValue::Int(meta.gid() as i64),
        )
    };
    #[cfg(not(unix))]
    let (mode, uid, gid) = (DynValue::Int(0), DynValue::Null, DynValue::Null);
    map.insert("mode".into(), mode);
    map.insert("uid".into(), uid);
    map.insert("gid".into(), gid);
    Ok(DynValue::Map(map))
}

/// `lchown`, not `chown`: config-weave runs as root and writes into
/// user-owned trees (a home directory), where following a link the user
/// planted would hand them ownership of an arbitrary file.
#[cfg(unix)]
fn chown(path: &str, owner: &DynValue, group: &DynValue) -> Result<(), String> {
    let uid = resolve_id(owner, "owner", lookup_uid)?;
    let gid = resolve_id(group, "group", lookup_gid)?;
    std::os::unix::fs::lchown(path, uid, gid).map_err(|e| format!("cannot chown {path}: {e}"))
}

#[cfg(not(unix))]
fn chown(_path: &str, _owner: &DynValue, _group: &DynValue) -> Result<(), String> {
    Err("fs::chown is only available on Unix".to_string())
}

/// Linux has no lchmod, so a final symlink is refused, for chown's reason.
/// The lstat gives the error a clear message; `fchmodat` with
/// `AT_SYMLINK_NOFOLLOW` closes the race if a link is swapped in after it.
#[cfg(unix)]
fn chmod(path: &str, mode: &DynValue) -> Result<(), String> {
    let mode = parse_mode(mode)?;
    let refuse = || format!("cannot chmod {path}: it is a symlink, which chmod does not follow");
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("cannot chmod {path}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(refuse());
    }
    let cpath = std::ffi::CString::new(path).map_err(|_| format!("invalid path '{path}'"))?;
    // SAFETY: `cpath` is a valid NUL-terminated string for the call's duration.
    let rc = unsafe {
        libc::fchmodat(
            libc::AT_FDCWD,
            cpath.as_ptr(),
            mode as libc::mode_t,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc == 0 {
        return Ok(());
    }
    let e = std::io::Error::last_os_error();
    // libc's fallback also says EOPNOTSUPP when /proc is not mounted.
    let swapped = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if e.raw_os_error() == Some(libc::EOPNOTSUPP) && swapped {
        return Err(refuse());
    }
    Err(format!("cannot chmod {path}: {e}"))
}

#[cfg(not(unix))]
fn chmod(_path: &str, _mode: &DynValue) -> Result<(), String> {
    Err("fs::chmod is only available on Unix".to_string())
}

/// An int is the mode itself; a string is octal digits, as chmod(1) reads
/// them, so "700" and "0700" agree.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_mode(v: &DynValue) -> Result<u32, String> {
    let (mode, shown) = match v {
        DynValue::Int(n) => (u32::try_from(*n).ok(), n.to_string()),
        DynValue::String(s) if !s.is_empty() && s.bytes().all(|b| (b'0'..=b'7').contains(&b)) => {
            (u32::from_str_radix(s, 8).ok(), format!("\"{s}\""))
        }
        other => {
            return Err(format!(
                "mode must be an int or a string of octal digits, got {other:?}"
            ));
        }
    };
    match mode {
        Some(m) if m <= 0o7777 => Ok(m),
        _ => Err(format!("mode {shown} is outside 0o7777")),
    }
}

/// A name is looked up first; an all-digit string that names no account
/// is taken as a numeric id, as chown(1) does.
#[cfg(unix)]
fn resolve_id(
    v: &DynValue,
    what: &str,
    lookup: fn(&str) -> Result<Option<u32>, String>,
) -> Result<Option<u32>, String> {
    match v {
        DynValue::Null => Ok(None),
        DynValue::Int(n) => u32::try_from(*n)
            .map(Some)
            .map_err(|_| format!("{what} id {n} is out of range")),
        DynValue::String(name) => match lookup(name)? {
            Some(id) => Ok(Some(id)),
            None => name
                .parse::<u32>()
                .map(Some)
                .map_err(|_| format!("unknown {what} '{name}'")),
        },
        other => Err(format!(
            "{what} must be a name, a numeric id or null, got {other:?}"
        )),
    }
}

/// Run a reentrant `get*nam_r` lookup, growing the scratch buffer on
/// ERANGE. Workers run scripts on parallel threads, so the non-reentrant
/// `getpwnam` is off the table.
#[cfg(unix)]
fn lookup_with<T>(
    name: &str,
    call: impl Fn(
        *const libc::c_char,
        *mut T,
        *mut libc::c_char,
        libc::size_t,
        *mut *mut T,
    ) -> libc::c_int,
    id: impl Fn(&T) -> u32,
) -> Result<Option<u32>, String> {
    let cname = std::ffi::CString::new(name).map_err(|_| format!("invalid name '{name}'"))?;
    let mut buf = vec![0 as libc::c_char; 1024];
    loop {
        // SAFETY: an all-zero passwd/group is a valid out-parameter; the
        // call only writes into it and into `buf`.
        let mut entry: T = unsafe { std::mem::zeroed() };
        let mut found: *mut T = std::ptr::null_mut();
        let rc = call(
            cname.as_ptr(),
            &mut entry,
            buf.as_mut_ptr(),
            buf.len(),
            &mut found,
        );
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 {
            return Err(format!(
                "cannot look up '{name}': {}",
                std::io::Error::from_raw_os_error(rc)
            ));
        }
        return Ok((!found.is_null()).then(|| id(&entry)));
    }
}

#[cfg(unix)]
fn lookup_uid(name: &str) -> Result<Option<u32>, String> {
    lookup_with(
        name,
        // SAFETY: arguments are valid pointers sized as passed.
        |n, e, b, l, r| unsafe { libc::getpwnam_r(n, e, b, l, r) },
        |p: &libc::passwd| p.pw_uid,
    )
}

#[cfg(unix)]
fn lookup_gid(name: &str) -> Result<Option<u32>, String> {
    lookup_with(
        name,
        // SAFETY: arguments are valid pointers sized as passed.
        |n, e, b, l, r| unsafe { libc::getgrnam_r(n, e, b, l, r) },
        |g: &libc::group| g.gr_gid,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexists_sees_a_dangling_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let link = dir.path().join("dangling");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(dir.path().join("missing"), &link).unwrap();
        let link = link.to_str().unwrap();
        assert!(!Path::new(link).exists(), "exists follows the link");
        assert!(lexists(link));
        assert!(!lexists(dir.path().join("missing").to_str().unwrap()));
    }

    #[cfg(unix)]
    fn meta_int(path: &str, key: &str) -> i64 {
        match metadata(path).unwrap() {
            DynValue::Map(m) => match m.get(key) {
                Some(DynValue::Int(n)) => *n,
                other => panic!("{key}: {other:?}"),
            },
            other => panic!("not a map: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn metadata_reports_permission_bits_and_owner() {
        use std::os::unix::fs::PermissionsExt;
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o4751)).unwrap();
        // Only the permission bits: no S_IFREG (0o100000) file-type bits.
        assert_eq!(meta_int(path, "mode"), 0o4751);
        // SAFETY: plain id queries with no arguments.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        assert_eq!(meta_int(path, "uid"), uid as i64);
        assert_eq!(meta_int(path, "gid"), gid as i64);
    }

    #[cfg(unix)]
    #[test]
    fn chmod_applies_int_and_octal_string_modes() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap();
        chmod(path, &DynValue::Int(0o700)).unwrap();
        assert_eq!(meta_int(path, "mode"), 0o700);
        chmod(path, &DynValue::String("0440".into())).unwrap();
        assert_eq!(meta_int(path, "mode"), 0o440);
        chmod(path, &DynValue::String("644".into())).unwrap();
        assert_eq!(meta_int(path, "mode"), 0o644);
    }

    #[test]
    fn parse_mode_rejects_bad_modes() {
        for (mode, want) in [
            (DynValue::Int(0o10000), "mode 4096 is outside 0o7777"),
            (DynValue::Int(-1), "mode -1 is outside 0o7777"),
            (
                DynValue::String("10000".into()),
                "mode \"10000\" is outside 0o7777",
            ),
        ] {
            assert_eq!(parse_mode(&mode).unwrap_err(), want);
        }
        for mode in [
            DynValue::String("0o700".into()),
            DynValue::String("789".into()),
            DynValue::String("".into()),
            DynValue::Null,
            DynValue::Bool(true),
        ] {
            let err = parse_mode(&mode).unwrap_err();
            assert!(err.starts_with("mode must be"), "{mode:?}: {err}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn chmod_refuses_a_final_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "x").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let link = link.to_str().unwrap();
        let err = chmod(link, &DynValue::Int(0o777)).unwrap_err();
        assert!(err.contains("is a symlink"), "{err}");
        assert!(err.contains(link), "{err}");
        assert_eq!(meta_int(target.to_str().unwrap(), "mode"), 0o600);
    }

    #[cfg(unix)]
    fn planted_link(dir: &Path) -> (std::path::PathBuf, String) {
        let target = dir.join("target");
        std::fs::write(&target, "original").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        (target, link.to_str().unwrap().to_string())
    }

    #[cfg(unix)]
    #[test]
    fn write_with_no_follow_refuses_a_planted_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let (target, link) = planted_link(dir.path());
        let opts = DynValue::Map(HashMap::from([(
            "follow_symlinks".to_string(),
            DynValue::Bool(false),
        )]));
        let err = write_with(&link, b"evil", &opts).unwrap_err();
        assert_eq!(err, refuse_symlink_write(&link));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        // A regular file is written normally, created when absent.
        let plain = dir.path().join("plain");
        let plain = plain.to_str().unwrap();
        write_with(plain, b"one", &opts).unwrap();
        write_with(plain, b"two", &opts).unwrap();
        assert_eq!(std::fs::read_to_string(plain).unwrap(), "two");
    }

    #[cfg(unix)]
    #[test]
    fn write_with_null_opts_writes_through_a_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let (target, link) = planted_link(dir.path());
        write_with(&link, b"through", &DynValue::Null).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "through");
    }

    #[test]
    fn write_opts_reject_unknown_keys_and_bad_values() {
        let bad = |k: &str, v: DynValue| {
            parse_write_opts(&DynValue::Map(HashMap::from([(k.to_string(), v)]))).unwrap_err()
        };
        assert_eq!(
            bad("follow_symlink", DynValue::Bool(false)),
            "unknown write option 'follow_symlink' (expected follow_symlinks)"
        );
        assert!(bad("follow_symlinks", DynValue::Int(0)).starts_with("follow_symlinks must be"));
        assert!(parse_write_opts(&DynValue::Int(1)).is_err());
        assert!(parse_write_opts(&DynValue::Null).unwrap());
    }

    #[cfg(unix)]
    fn id_name(flag: &str) -> String {
        let out = std::process::Command::new("id").arg(flag).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn chown_to_self_by_id_and_by_name() {
        use std::os::unix::fs::MetadataExt;
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap();
        // SAFETY: plain id queries with no arguments.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        chown(path, &DynValue::Int(uid as i64), &DynValue::Int(gid as i64)).unwrap();
        chown(
            path,
            &DynValue::String(id_name("-un")),
            &DynValue::String(id_name("-gn")),
        )
        .unwrap();
        chown(path, &DynValue::Null, &DynValue::Null).unwrap();
        let meta = std::fs::metadata(path).unwrap();
        assert_eq!((meta.uid(), meta.gid()), (uid, gid));
    }

    #[cfg(unix)]
    #[test]
    fn chown_unknown_user_errors() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap();
        let err = chown(
            path,
            &DynValue::String("no-such-user-cw".into()),
            &DynValue::Null,
        )
        .unwrap_err();
        assert_eq!(err, "unknown owner 'no-such-user-cw'");
        let err = chown(path, &DynValue::Null, &DynValue::Bool(true)).unwrap_err();
        assert!(err.starts_with("group must be"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn chown_does_not_follow_a_final_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
        // SAFETY: plain id query with no arguments.
        let uid = unsafe { libc::getuid() };
        chown(
            link.to_str().unwrap(),
            &DynValue::Int(uid as i64),
            &DynValue::Null,
        )
        .unwrap();
    }
}
