//! The `acl` module: read, compare and set the owner and DACL of Windows
//! files, directories and registry keys, so a package does not shell out to
//! PowerShell `Get-Acl`/`Set-Acl`. `set` replaces every explicit entry and
//! never writes inherited ones; `diff` makes exactly the comparison `set`
//! converges, so a check script can call it. Registered on every platform;
//! calls fail at runtime off Windows.

use wscript::Module;
use wscript_std::DynValue;

#[cfg(not(windows))]
const NOT_WINDOWS: &str = "the 'acl' module is only available on Windows";

/// Everything that needs no Windows call: option and spec parsing, the
/// rights name tables, binary SID and ACL encoding, and the comparison
/// `diff` reports. Compiles and is unit-tested on every platform; only `win`
/// uses it.
mod codec {
    #![cfg_attr(not(windows), allow(dead_code))]

    use std::collections::{HashMap, HashSet};
    use std::fmt;

    use wscript_std::DynValue;

    use crate::hostapi::registry::codec::{View, parse_view};

    // ACE header flags (winnt.h).
    const OBJECT_INHERIT_ACE: u8 = 0x01;
    const CONTAINER_INHERIT_ACE: u8 = 0x02;
    const NO_PROPAGATE_INHERIT_ACE: u8 = 0x04;
    const INHERIT_ONLY_ACE: u8 = 0x08;
    const INHERITED_ACE: u8 = 0x10;

    // ACE types (winnt.h).
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE: u8 = 1;

    // ACL_REVISION: the revision for ACLs holding only standard ACEs.
    const ACL_REVISION: u8 = 2;
    const ACL_HEADER_LEN: usize = 8;
    // An ACCESS_ALLOWED_ACE / ACCESS_DENIED_ACE before its SID: header + mask.
    const ACE_FIXED_LEN: usize = 8;

    // Generic access bits (winnt.h), mapped to specific rights on read.
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const GENERIC_EXECUTE: u32 = 0x2000_0000;
    const GENERIC_ALL: u32 = 0x1000_0000;

    /// A binary SID, in the layout Windows stores it: revision, subauthority
    /// count, 48-bit big-endian identifier authority, little-endian 32-bit
    /// subauthorities. Equality is SID equality.
    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    pub(super) struct Sid(Vec<u8>);

    impl Sid {
        /// Parse the binary SID at the start of `buf`, which may run past it.
        pub(super) fn from_bytes(buf: &[u8]) -> Result<Sid, String> {
            if buf.len() < 8 || buf[0] != 1 {
                return Err("malformed SID".to_string());
            }
            let len = 8 + 4 * buf[1] as usize;
            if buf[1] > 15 || buf.len() < len {
                return Err("malformed SID".to_string());
            }
            Ok(Sid(buf[..len].to_vec()))
        }

        /// Parse `S-1-<authority>-<sub>...`, as ConvertStringSidToSidW does
        /// for the numeric form. The authority may be decimal or `0x` hex.
        pub(super) fn parse(text: &str) -> Result<Sid, String> {
            let bad = || format!("'{text}' is not a valid SID string");
            let mut parts = text.split('-');
            if !parts.next().is_some_and(|p| p.eq_ignore_ascii_case("s"))
                || parts.next() != Some("1")
            {
                return Err(bad());
            }
            let authority = parts.next().ok_or_else(bad)?;
            let authority = match authority
                .strip_prefix("0x")
                .or_else(|| authority.strip_prefix("0X"))
            {
                Some(hex) => u64::from_str_radix(hex, 16).map_err(|_| bad())?,
                None => authority.parse::<u64>().map_err(|_| bad())?,
            };
            if authority >= 1 << 48 {
                return Err(bad());
            }
            let subs = parts
                .map(|p| {
                    if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(bad());
                    }
                    p.parse::<u32>().map_err(|_| bad())
                })
                .collect::<Result<Vec<u32>, String>>()?;
            if subs.len() > 15 {
                return Err(bad());
            }
            let mut bytes = vec![1, subs.len() as u8];
            bytes.extend_from_slice(&authority.to_be_bytes()[2..]);
            for s in subs {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            Ok(Sid(bytes))
        }

        pub(super) fn bytes(&self) -> &[u8] {
            &self.0
        }
    }

    impl fmt::Display for Sid {
        /// The string form ConvertSidToStringSidW produces: the authority in
        /// decimal below 2^32, otherwise as `0x` and twelve hex digits.
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let mut auth = [0u8; 8];
            auth[2..].copy_from_slice(&self.0[2..8]);
            let auth = u64::from_be_bytes(auth);
            if auth < 1 << 32 {
                write!(f, "S-1-{auth}")?;
            } else {
                write!(f, "S-1-0x{auth:012X}")?;
            }
            for sub in self.0[8..].as_chunks::<4>().0 {
                write!(f, "-{}", u32::from_le_bytes(*sub))?;
            }
            Ok(())
        }
    }

    /// What a principal string in a spec names, before any lookup.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum PrincipalRef {
        /// A SID string, `S-1-5-32-544`.
        Sid(Sid),
        /// `.\name`: an account on this computer. LookupAccountNameW does not
        /// understand `.`, so `win` substitutes the computer name.
        Local(String),
        /// Any other account name, passed to LookupAccountNameW as written.
        Account(String),
    }

    pub(super) fn classify(principal: &str) -> Result<PrincipalRef, String> {
        let p = principal.trim();
        if p.is_empty() {
            return Err("a principal must not be empty".to_string());
        }
        // A SID string is `S-` then a digit; an account named `S-Admins`
        // stays an account name.
        let b = p.as_bytes();
        if b.len() > 2 && b[0].eq_ignore_ascii_case(&b's') && b[1] == b'-' && b[2].is_ascii_digit()
        {
            return Sid::parse(p).map(PrincipalRef::Sid);
        }
        if let Some(name) = p.strip_prefix(".\\") {
            if name.is_empty() {
                return Err(format!("'{principal}' names no account"));
            }
            return Ok(PrincipalRef::Local(name.to_string()));
        }
        Ok(PrincipalRef::Account(p.to_string()))
    }

    /// What `acl` operates on, chosen by `opts`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Target {
        File,
        Registry(View),
    }

    impl Target {
        pub(super) fn rights(self) -> &'static Rights {
            match self {
                Target::File => &FILE_RIGHTS,
                Target::Registry(_) => &REGISTRY_RIGHTS,
            }
        }
    }

    /// Parse the trailing `opts`: `Null` or `{target, view}`.
    pub(super) fn parse_opts(opts: &DynValue) -> Result<Target, String> {
        let m = match opts {
            DynValue::Null => return Ok(Target::File),
            DynValue::Map(m) => m,
            other => return Err(format!("acl options must be a map or null, got {other:?}")),
        };
        let mut registry = false;
        let mut view = None;
        for (k, v) in m {
            match (k.as_str(), v) {
                ("target", DynValue::String(s)) => {
                    registry = match s.as_str() {
                        "file" => false,
                        "registry" => true,
                        other => {
                            return Err(format!(
                                "unknown acl target '{other}' (expected file, registry)"
                            ));
                        }
                    }
                }
                ("target", other) => {
                    return Err(format!("acl target must be a string, got {other:?}"));
                }
                ("view", v) => view = Some(parse_view(v)?),
                (other, _) => {
                    return Err(format!(
                        "unknown acl option '{other}' (expected target, view)"
                    ));
                }
            }
        }
        match (registry, view) {
            (true, view) => Ok(Target::Registry(view.unwrap_or_default())),
            (false, None) => Ok(Target::File),
            (false, Some(_)) => {
                Err("acl option 'view' is only valid with target \"registry\"".to_string())
            }
        }
    }

    /// The rights vocabulary of one target: the .NET FileSystemRights or
    /// RegistryRights names in snake_case.
    pub(super) struct Rights {
        /// Multi-bit names, in the order `names` tries them: most bits first.
        composites: &'static [(&'static str, u32)],
        /// One name per bit, in bit order: the names `names` reports.
        singles: &'static [(&'static str, u32)],
        /// Accepted in a spec, never reported (`list_directory` is
        /// `read_data`).
        aliases: &'static [(&'static str, u32)],
        /// What GENERIC_READ, _WRITE, _EXECUTE and _ALL mean for the target.
        generic: [u32; 4],
    }

    pub(super) static FILE_RIGHTS: Rights = Rights {
        composites: &[
            ("full_control", 0x001F_01FF),
            ("modify", 0x0003_01BF),
            ("read_and_execute", 0x0002_00A9),
            ("read", 0x0002_0089),
            ("write", 0x0000_0116),
        ],
        singles: &[
            ("read_data", 0x0001),
            ("write_data", 0x0002),
            ("append_data", 0x0004),
            ("read_extended_attributes", 0x0008),
            ("write_extended_attributes", 0x0010),
            ("execute_file", 0x0020),
            ("delete_subdirectories_and_files", 0x0040),
            ("read_attributes", 0x0080),
            ("write_attributes", 0x0100),
            ("delete", 0x0001_0000),
            ("read_permissions", 0x0002_0000),
            ("change_permissions", 0x0004_0000),
            ("take_ownership", 0x0008_0000),
            ("synchronize", 0x0010_0000),
        ],
        aliases: &[
            ("list_directory", 0x0001),
            ("create_files", 0x0002),
            ("create_directories", 0x0004),
            ("traverse", 0x0020),
        ],
        // FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_GENERIC_EXECUTE,
        // FILE_ALL_ACCESS.
        generic: [0x0012_0089, 0x0012_0116, 0x0012_00A0, 0x001F_01FF],
    };

    pub(super) static REGISTRY_RIGHTS: Rights = Rights {
        composites: &[
            ("full_control", 0x000F_003F),
            ("read_key", 0x0002_0019),
            ("write_key", 0x0002_0006),
        ],
        singles: &[
            ("query_values", 0x0001),
            ("set_value", 0x0002),
            ("create_subkey", 0x0004),
            ("enumerate_subkeys", 0x0008),
            ("notify", 0x0010),
            ("create_link", 0x0020),
            ("delete", 0x0001_0000),
            ("read_permissions", 0x0002_0000),
            ("change_permissions", 0x0004_0000),
            ("take_ownership", 0x0008_0000),
        ],
        aliases: &[("execute_key", 0x0002_0019)],
        // KEY_READ, KEY_WRITE, KEY_EXECUTE, KEY_ALL_ACCESS.
        generic: [0x0002_0019, 0x0002_0006, 0x0002_0019, 0x000F_003F],
    };

    impl Rights {
        /// Replace any GENERIC_* bits with the specific rights they grant on
        /// this target, as the object manager does at access-check time.
        pub(super) fn map_generic(&self, mask: u32) -> u32 {
            let generic = [GENERIC_READ, GENERIC_WRITE, GENERIC_EXECUTE, GENERIC_ALL];
            let mut out = mask & !(GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | GENERIC_ALL);
            for (bit, specific) in generic.iter().zip(self.generic) {
                if mask & bit != 0 {
                    out |= specific;
                }
            }
            out
        }

        /// The names for a mask: each composite (most bits first) whose bits
        /// are all in `mask` and which covers a bit no earlier name did, then
        /// the single name of each bit still uncovered, in bit order. Bits
        /// with no name stay visible only in the mask.
        pub(super) fn names(&self, mask: u32) -> Vec<&'static str> {
            let mut out = Vec::new();
            let mut left = mask;
            for &(name, bits) in self.composites {
                if mask & bits == bits && left & bits != 0 {
                    out.push(name);
                    left &= !bits;
                }
            }
            for &(name, bit) in self.singles {
                if left & bit != 0 {
                    out.push(name);
                    left &= !bit;
                }
            }
            out
        }

        fn lookup(&self, name: &str) -> Option<u32> {
            self.composites
                .iter()
                .chain(self.singles)
                .chain(self.aliases)
                .find(|(n, _)| *n == name)
                .map(|&(_, bits)| bits)
        }

        fn valid_names(&self) -> String {
            self.composites
                .iter()
                .chain(self.singles)
                .chain(self.aliases)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    const INHERIT_FLAGS: [(&str, u8); 4] = [
        ("object", OBJECT_INHERIT_ACE),
        ("container", CONTAINER_INHERIT_ACE),
        ("no_propagate", NO_PROPAGATE_INHERIT_ACE),
        ("inherit_only", INHERIT_ONLY_ACE),
    ];

    pub(super) fn inherit_names(flags: u8) -> Vec<&'static str> {
        INHERIT_FLAGS
            .iter()
            .filter(|(_, bit)| flags & bit != 0)
            .map(|(n, _)| *n)
            .collect()
    }

    /// The type of an ACE. A DACL can hold others (object, callback ACEs);
    /// `acl` reports them but cannot write them.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(super) enum Kind {
        Allow,
        Deny,
        Other(u8),
    }

    impl Kind {
        fn name(self) -> &'static str {
            match self {
                Kind::Allow => "allow",
                Kind::Deny => "deny",
                Kind::Other(_) => "other",
            }
        }
    }

    /// One ACE as stored. `sid` is None only for an `Other` ACE.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub(super) struct Ace {
        pub(super) kind: Kind,
        pub(super) flags: u8,
        pub(super) mask: u32,
        pub(super) sid: Option<Sid>,
    }

    impl Ace {
        fn inherited(&self) -> bool {
            self.flags & INHERITED_ACE != 0
        }
    }

    /// A security descriptor's owner and DACL. `dacl` is None for a NULL
    /// DACL, which grants everyone full access.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Descriptor {
        pub(super) owner: Option<Sid>,
        pub(super) protected: bool,
        pub(super) dacl: Option<Vec<Ace>>,
    }

    impl Descriptor {
        /// Every SID a reader wants a name for.
        pub(super) fn sids(&self) -> Vec<&Sid> {
            let mut out: Vec<&Sid> = self.owner.iter().collect();
            for ace in self.dacl.iter().flatten() {
                out.extend(ace.sid.as_ref());
            }
            out
        }
    }

    /// The ACEs of a binary ACL, in stored order.
    pub(super) fn parse_acl(buf: &[u8]) -> Result<Vec<Ace>, String> {
        if buf.len() < ACL_HEADER_LEN {
            return Err("malformed ACL: short header".to_string());
        }
        let size = u16::from_le_bytes([buf[2], buf[3]]) as usize;
        let count = u16::from_le_bytes([buf[4], buf[5]]) as usize;
        let buf = buf
            .get(..size)
            .ok_or_else(|| "malformed ACL: size past the buffer".to_string())?;
        let mut aces = Vec::with_capacity(count);
        let mut at = ACL_HEADER_LEN;
        for _ in 0..count {
            let hdr = buf
                .get(at..at + 4)
                .ok_or_else(|| "malformed ACL: ACE past the end".to_string())?;
            let ace_size = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
            let body = buf
                .get(at..at + ace_size)
                .filter(|b| b.len() >= ACE_FIXED_LEN)
                .ok_or_else(|| "malformed ACL: ACE past the end".to_string())?;
            let mask = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            let (kind, sid) = match hdr[0] {
                ACCESS_ALLOWED_ACE_TYPE => (Kind::Allow, Some(Sid::from_bytes(&body[8..])?)),
                ACCESS_DENIED_ACE_TYPE => (Kind::Deny, Some(Sid::from_bytes(&body[8..])?)),
                other => (Kind::Other(other), None),
            };
            aces.push(Ace {
                kind,
                flags: hdr[1],
                mask,
                sid,
            });
            at += ace_size;
        }
        Ok(aces)
    }

    /// A binary ACL holding `aces` in the order given. Only allow and deny
    /// ACEs can be encoded.
    pub(super) fn build_acl(aces: &[Ace]) -> Result<Vec<u8>, String> {
        let mut body = Vec::new();
        for ace in aces {
            let kind = match ace.kind {
                Kind::Allow => ACCESS_ALLOWED_ACE_TYPE,
                Kind::Deny => ACCESS_DENIED_ACE_TYPE,
                Kind::Other(t) => return Err(format!("cannot write an ACE of type {t}")),
            };
            let sid = ace
                .sid
                .as_ref()
                .ok_or_else(|| "an allow or deny ACE needs a SID".to_string())?;
            let size = ACE_FIXED_LEN + sid.bytes().len();
            body.push(kind);
            body.push(ace.flags);
            body.extend_from_slice(&(size as u16).to_le_bytes());
            body.extend_from_slice(&ace.mask.to_le_bytes());
            body.extend_from_slice(sid.bytes());
        }
        let size = ACL_HEADER_LEN + body.len();
        let size = u16::try_from(size).map_err(|_| "the ACL exceeds 64 KiB".to_string())?;
        let count = aces.len() as u16;
        let mut acl = vec![ACL_REVISION, 0];
        acl.extend_from_slice(&size.to_le_bytes());
        acl.extend_from_slice(&count.to_le_bytes());
        acl.extend_from_slice(&[0, 0]);
        acl.extend(body);
        Ok(acl)
    }

    /// Resolved names, by SID: None when a SID does not resolve.
    pub(super) type Names = HashMap<Sid, Option<String>>;

    fn principal_value(sid: &Sid, names: &Names) -> (DynValue, DynValue) {
        let name = match names.get(sid) {
            Some(Some(n)) => DynValue::String(n.clone()),
            _ => DynValue::Null,
        };
        (DynValue::String(sid.to_string()), name)
    }

    /// `{sid, name}`: `read`'s owner, and what `lookup` returns.
    pub(super) fn principal_map(sid: &Sid, names: &Names) -> DynValue {
        let (sid, name) = principal_value(sid, names);
        DynValue::Map(HashMap::from([("sid".into(), sid), ("name".into(), name)]))
    }

    fn str_list(items: Vec<&str>) -> DynValue {
        DynValue::List(
            items
                .into_iter()
                .map(|s| DynValue::String(s.into()))
                .collect(),
        )
    }

    /// The map `acl::read` returns. Masks are reported with GENERIC_* bits
    /// mapped to the target's rights.
    pub(super) fn to_value(desc: &Descriptor, target: Target, names: &Names) -> DynValue {
        let rights = target.rights();
        let owner = match &desc.owner {
            Some(sid) => principal_map(sid, names),
            None => DynValue::Null,
        };
        let entries = desc
            .dacl
            .iter()
            .flatten()
            .map(|ace| {
                let mask = rights.map_generic(ace.mask);
                let (sid, name) = match &ace.sid {
                    Some(sid) => principal_value(sid, names),
                    None => (DynValue::Null, DynValue::Null),
                };
                let mut m = HashMap::from([
                    ("sid".to_string(), sid),
                    ("name".to_string(), name),
                    ("kind".to_string(), DynValue::String(ace.kind.name().into())),
                    ("rights".to_string(), str_list(rights.names(mask))),
                    ("mask".to_string(), DynValue::Int(i64::from(mask))),
                    ("inherit".to_string(), str_list(inherit_names(ace.flags))),
                    ("inherited".to_string(), DynValue::Bool(ace.inherited())),
                ]);
                if let Kind::Other(t) = ace.kind {
                    m.insert("type".to_string(), DynValue::Int(i64::from(t)));
                }
                DynValue::Map(m)
            })
            .collect();
        DynValue::Map(HashMap::from([
            ("owner".to_string(), owner),
            ("protected".to_string(), DynValue::Bool(desc.protected)),
            // A NULL DACL grants everyone full access and an empty one
            // grants nobody anything; both have no entries, so this key
            // is what tells them apart.
            ("null_dacl".to_string(), DynValue::Bool(desc.dacl.is_none())),
            ("entries".to_string(), DynValue::List(entries)),
        ]))
    }

    /// A parsed `set`/`diff` spec. `P` is the principal: the string as
    /// written, then a `Principal` once `win` has looked it up.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Spec<P> {
        pub(super) owner: Option<P>,
        pub(super) protected: Option<bool>,
        pub(super) entries: Vec<Entry<P>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Entry<P> {
        pub(super) principal: P,
        pub(super) kind: Kind,
        /// Rights and mask united, GENERIC_* bits mapped.
        pub(super) mask: u32,
        /// Inherit flags only; never INHERITED_ACE.
        pub(super) flags: u8,
    }

    /// A looked-up principal: its SID, and what diff messages call it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Principal {
        pub(super) sid: Sid,
        pub(super) label: String,
    }

    impl<P> Spec<P> {
        /// Look up every principal with `f`, failing on the first that does
        /// not resolve.
        pub(super) fn resolve<Q>(
            self,
            mut f: impl FnMut(&P) -> Result<Q, String>,
        ) -> Result<Spec<Q>, String> {
            Ok(Spec {
                owner: self.owner.as_ref().map(&mut f).transpose()?,
                protected: self.protected,
                entries: self
                    .entries
                    .into_iter()
                    .map(|e| {
                        Ok(Entry {
                            principal: f(&e.principal)?,
                            kind: e.kind,
                            mask: e.mask,
                            flags: e.flags,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            })
        }
    }

    fn string_list(what: &str, v: &DynValue) -> Result<Vec<String>, String> {
        let DynValue::List(items) = v else {
            return Err(format!("{what} must be a list of strings, got {v:?}"));
        };
        items
            .iter()
            .map(|i| match i {
                DynValue::String(s) => Ok(s.clone()),
                other => Err(format!("{what} must be a list of strings, got {other:?}")),
            })
            .collect()
    }

    fn parse_entry(i: usize, v: &DynValue, rights: &Rights) -> Result<Entry<String>, String> {
        let DynValue::Map(m) = v else {
            return Err(format!("entries[{i}] must be a map, got {v:?}"));
        };
        let mut principal = None;
        let mut kind = None;
        let mut mask = 0u32;
        let mut has_rights = false;
        let mut flags = 0u8;
        for (k, v) in m {
            match (k.as_str(), v) {
                ("principal", DynValue::String(s)) => principal = Some(s.clone()),
                ("kind", DynValue::String(s)) => {
                    kind = Some(match s.as_str() {
                        "allow" => Kind::Allow,
                        "deny" => Kind::Deny,
                        other => {
                            return Err(format!(
                                "entries[{i}].kind '{other}' is not allow or deny"
                            ));
                        }
                    })
                }
                ("rights", v) => {
                    has_rights = true;
                    for name in string_list(&format!("entries[{i}].rights"), v)? {
                        mask |= rights.lookup(&name).ok_or_else(|| {
                            format!(
                                "entries[{i}]: unknown right '{name}' (valid: {})",
                                rights.valid_names()
                            )
                        })?;
                    }
                }
                ("mask", DynValue::Int(n)) => {
                    has_rights = true;
                    mask |= u32::try_from(*n).map_err(|_| {
                        format!("entries[{i}].mask {n} is not a 32-bit access mask")
                    })?;
                }
                ("inherit", v) => {
                    for name in string_list(&format!("entries[{i}].inherit"), v)? {
                        flags |= INHERIT_FLAGS
                            .iter()
                            .find(|(n, _)| *n == name)
                            .map(|&(_, bit)| bit)
                            .ok_or_else(|| {
                                format!(
                                    "entries[{i}]: unknown inherit flag '{name}' \
                                     (valid: object, container, no_propagate, inherit_only)"
                                )
                            })?;
                    }
                }
                ("principal" | "kind" | "mask", other) => {
                    return Err(format!("entries[{i}].{k} has the wrong type: {other:?}"));
                }
                (other, _) => {
                    return Err(format!(
                        "entries[{i}]: unknown key '{other}' \
                         (expected principal, kind, rights, mask, inherit)"
                    ));
                }
            }
        }
        let principal = principal.ok_or_else(|| format!("entries[{i}] has no principal"))?;
        let kind = kind.ok_or_else(|| format!("entries[{i}] ({principal}) has no kind"))?;
        if !has_rights {
            return Err(format!("entries[{i}] ({principal}) needs rights or mask"));
        }
        let mask = rights.map_generic(mask);
        if mask == 0 {
            return Err(format!("entries[{i}] ({principal}) grants no rights"));
        }
        if flags & (NO_PROPAGATE_INHERIT_ACE | INHERIT_ONLY_ACE) != 0
            && flags & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) == 0
        {
            return Err(format!(
                "entries[{i}] ({principal}): no_propagate and inherit_only need object or container"
            ));
        }
        Ok(Entry {
            principal,
            kind,
            mask,
            flags,
        })
    }

    /// Parse a spec: `{owner?, protected?, entries}`.
    pub(super) fn parse_spec(spec: &DynValue, target: Target) -> Result<Spec<String>, String> {
        let DynValue::Map(m) = spec else {
            return Err(format!("an acl spec must be a map, got {spec:?}"));
        };
        let mut out = Spec {
            owner: None,
            protected: None,
            entries: Vec::new(),
        };
        let mut has_entries = false;
        for (k, v) in m {
            match (k.as_str(), v) {
                ("owner", DynValue::String(s)) => out.owner = Some(s.clone()),
                ("owner", DynValue::Null) => {}
                ("protected", DynValue::Bool(b)) => out.protected = Some(*b),
                ("protected", DynValue::Null) => {}
                ("entries", DynValue::List(items)) => {
                    has_entries = true;
                    out.entries = items
                        .iter()
                        .enumerate()
                        .map(|(i, e)| parse_entry(i, e, target.rights()))
                        .collect::<Result<_, String>>()?;
                }
                ("owner" | "protected" | "entries", other) => {
                    return Err(format!("acl spec '{k}' has the wrong type: {other:?}"));
                }
                (other, _) => {
                    return Err(format!(
                        "unknown acl spec key '{other}' (expected owner, protected, entries)"
                    ));
                }
            }
        }
        if !has_entries {
            return Err(
                "acl spec needs 'entries' (pass [] to remove every explicit entry)".to_string(),
            );
        }
        Ok(out)
    }

    /// Inherit flags mean something only on a container (a directory or a
    /// registry key). Windows strips them from the ACEs of a plain file, so
    /// a spec asking for them there could never converge: an `Err`.
    pub(super) fn check_inherit(
        spec: &Spec<Principal>,
        container: bool,
        path: &str,
    ) -> Result<(), String> {
        match spec.entries.iter().find(|e| e.flags != 0) {
            Some(e) if !container => Err(format!(
                "the entry for {} has inherit flags, but '{path}' is a file: only \
                 directories and registry keys pass entries on",
                e.principal.label
            )),
            _ => Ok(()),
        }
    }

    /// The explicit ACEs `set` writes for `spec`: duplicates dropped, deny
    /// entries before allow entries (Windows canonical order), each group in
    /// spec order.
    pub(super) fn explicit_aces(spec: &Spec<Principal>) -> Vec<Ace> {
        let mut seen = HashSet::new();
        let mut deny = Vec::new();
        let mut allow = Vec::new();
        for e in &spec.entries {
            let ace = Ace {
                kind: e.kind,
                flags: e.flags,
                mask: e.mask,
                sid: Some(e.principal.sid.clone()),
            };
            if seen.insert(ace.clone()) {
                match e.kind {
                    Kind::Deny => deny.push(ace),
                    _ => allow.push(ace),
                }
            }
        }
        deny.extend(allow);
        deny
    }

    fn describe(ace: &Ace, label: &str, rights: &Rights) -> String {
        let mask = rights.map_generic(ace.mask);
        let who = match (&ace.sid, ace.kind) {
            (Some(sid), _) if label.is_empty() => sid.to_string(),
            (Some(sid), _) => format!("{label} ({sid})"),
            (None, Kind::Other(t)) => format!("of ACE type {t}"),
            (None, _) => "with no SID".to_string(),
        };
        let inherit = inherit_names(ace.flags);
        format!(
            "{} {who} rights [{}] mask 0x{mask:X} inherit [{}]",
            ace.kind.name(),
            rights.names(mask).join(", "),
            inherit.join(", ")
        )
    }

    fn label(sid: &Sid, names: &Names) -> String {
        names.get(sid).cloned().flatten().unwrap_or_default()
    }

    /// The comparison `set` converges, as human-readable lines; empty means
    /// converged. The owner is compared by SID, `protected` when the spec
    /// gives it, and explicit entries as a set of (SID, kind, mask with
    /// GENERIC_* mapped, inherit flags). Inherited entries count only when
    /// the spec asks for a protected DACL and the DACL already is one.
    pub(super) fn diff(
        current: &Descriptor,
        names: &Names,
        spec: &Spec<Principal>,
        target: Target,
    ) -> Vec<String> {
        let rights = target.rights();
        let mut out = Vec::new();
        if let Some(want) = &spec.owner
            && current.owner.as_ref() != Some(&want.sid)
        {
            let is = match &current.owner {
                Some(sid) => match label(sid, names) {
                    l if l.is_empty() => sid.to_string(),
                    l => format!("{l} ({sid})"),
                },
                None => "unset".to_string(),
            };
            out.push(format!("owner is {is}, want {} ({})", want.label, want.sid));
        }
        if let Some(want) = spec.protected
            && current.protected != want
        {
            out.push(if want {
                "inheritance is enabled, want it disabled (protected: true)".to_string()
            } else {
                "inheritance is disabled, want it enabled (protected: false)".to_string()
            });
        }
        let Some(dacl) = &current.dacl else {
            out.push("the DACL is NULL, which grants everyone full access".to_string());
            return out;
        };
        let normalise = |ace: &Ace| Ace {
            mask: rights.map_generic(ace.mask),
            ..ace.clone()
        };
        let have: HashSet<Ace> = dacl
            .iter()
            .filter(|a| !a.inherited())
            .map(normalise)
            .collect();
        let want = explicit_aces(spec);
        let want_set: HashSet<&Ace> = want.iter().collect();
        for ace in &want {
            if !have.contains(ace) {
                // Name it as the spec entry that produced it does.
                let l = spec
                    .entries
                    .iter()
                    .find(|e| Some(&e.principal.sid) == ace.sid.as_ref())
                    .map(|e| e.principal.label.as_str())
                    .unwrap_or_default();
                out.push(format!("missing entry: {}", describe(ace, l, rights)));
            }
        }
        let mut reported = HashSet::new();
        for ace in dacl.iter().filter(|a| !a.inherited()) {
            let n = normalise(ace);
            if !want_set.contains(&n) && reported.insert(n.clone()) {
                let l = ace
                    .sid
                    .as_ref()
                    .map(|s| label(s, names))
                    .unwrap_or_default();
                out.push(format!("unexpected entry: {}", describe(ace, &l, rights)));
            }
        }
        if spec.protected == Some(true) && current.protected {
            for ace in dacl.iter().filter(|a| a.inherited()) {
                let l = ace
                    .sid
                    .as_ref()
                    .map(|s| label(s, names))
                    .unwrap_or_default();
                out.push(format!(
                    "unexpected inherited entry: {}",
                    describe(ace, &l, rights)
                ));
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn sid(s: &str) -> Sid {
            Sid::parse(s).unwrap()
        }

        fn s(v: &str) -> DynValue {
            DynValue::String(v.into())
        }

        fn map(pairs: &[(&str, DynValue)]) -> DynValue {
            DynValue::Map(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            )
        }

        fn list(items: &[&str]) -> DynValue {
            DynValue::List(items.iter().map(|i| s(i)).collect())
        }

        fn principal(text: &str) -> Principal {
            Principal {
                sid: sid(text),
                label: format!("<{text}>"),
            }
        }

        fn resolved(spec: Spec<String>) -> Spec<Principal> {
            spec.resolve(|p| Ok(principal(p))).unwrap()
        }

        #[test]
        fn sid_strings_round_trip_through_the_binary_form() {
            for text in [
                "S-1-5-32-544",
                "S-1-5-18",
                "S-1-1-0",
                "S-1-3-0",
                "S-1-5-21-1004336348-1177238915-682003330-512",
                "S-1-15-2-1",
                "S-1-0x123456789ABC-7",
            ] {
                assert_eq!(sid(text).to_string(), text);
            }
            // Administrators, as Windows stores it.
            assert_eq!(
                sid("S-1-5-32-544").bytes(),
                [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 2, 0, 0]
            );
            assert_eq!(sid("s-1-5-18").to_string(), "S-1-5-18");
        }

        #[test]
        fn malformed_sid_strings_are_errors() {
            for bad in [
                "S-2-5-18",
                "S-1",
                "S-1-x",
                "S-1-5-",
                "S-1-5-+3",
                "S-1-5-4294967296",
                "S-1-281474976710656",
                "S-1-5-1-2-3-4-5-6-7-8-9-10-11-12-13-14-15-16",
            ] {
                assert!(Sid::parse(bad).is_err(), "{bad}");
            }
            assert!(Sid::from_bytes(&[1, 2, 0, 0, 0, 0, 0, 5, 32]).is_err());
            assert!(Sid::from_bytes(&[2, 0, 0, 0, 0, 0, 0, 5]).is_err());
        }

        #[test]
        fn principals_are_classified_before_lookup() {
            assert_eq!(
                classify("S-1-5-32-544").unwrap(),
                PrincipalRef::Sid(sid("S-1-5-32-544"))
            );
            assert_eq!(
                classify(".\\cwtest").unwrap(),
                PrincipalRef::Local("cwtest".into())
            );
            for name in [
                "BUILTIN\\Administrators",
                "Administrators",
                "NT AUTHORITY\\SYSTEM",
                "DOMAIN\\user",
                "S-Admins",
            ] {
                assert_eq!(
                    classify(name).unwrap(),
                    PrincipalRef::Account(name.into()),
                    "{name}"
                );
            }
            assert!(classify("  ").is_err());
            assert!(classify(".\\").is_err());
            assert!(
                classify("S-1-5-bogus")
                    .unwrap_err()
                    .contains("not a valid SID")
            );
        }

        #[test]
        fn full_control_reads_back_as_one_name() {
            assert_eq!(FILE_RIGHTS.names(0x1F01FF), ["full_control"]);
            assert_eq!(REGISTRY_RIGHTS.names(0xF003F), ["full_control"]);
        }

        #[test]
        fn names_prefer_composites_then_single_rights() {
            let f = &FILE_RIGHTS;
            // Users on a fresh file: ReadAndExecute, Synchronize.
            assert_eq!(f.names(0x1200A9), ["read_and_execute", "synchronize"]);
            // Authenticated Users: Modify, Synchronize.
            assert_eq!(f.names(0x1301BF), ["modify", "synchronize"]);
            assert_eq!(f.names(0x20089 | 0x116), ["read", "write"]);
            assert_eq!(f.names(0x200A9 | 0x116), ["read_and_execute", "write"]);
            assert_eq!(f.names(0x1 | 0x20), ["read_data", "execute_file"]);
            assert_eq!(f.names(0x10000), ["delete"]);
            // A bit with no name is left to the mask.
            assert_eq!(f.names(0x0100_0000), Vec::<&str>::new());
            assert_eq!(f.names(0), Vec::<&str>::new());
            let r = &REGISTRY_RIGHTS;
            // read_key and write_key overlap on read_permissions; both are
            // reported because each covers bits the other does not.
            assert_eq!(r.names(0x2001F), ["read_key", "write_key"]);
            assert_eq!(r.names(0x20019 | 0x10000), ["read_key", "delete"]);
            assert_eq!(r.names(0x1), ["query_values"]);
        }

        #[test]
        fn names_round_trip_to_the_same_mask() {
            for (rights, mask) in [
                (&FILE_RIGHTS, 0x1200A9u32),
                (&FILE_RIGHTS, 0x1301BF),
                (&FILE_RIGHTS, 0x1F01FF),
                (&FILE_RIGHTS, 0x0001_0041),
                (&REGISTRY_RIGHTS, 0x2001F),
                (&REGISTRY_RIGHTS, 0xF003F),
            ] {
                let back = rights
                    .names(mask)
                    .iter()
                    .fold(0, |m, n| m | rights.lookup(n).unwrap());
                assert_eq!(back, mask);
            }
        }

        #[test]
        fn generic_bits_map_to_the_targets_rights() {
            // CREATOR OWNER on a directory: GENERIC_ALL, inherit-only.
            assert_eq!(FILE_RIGHTS.map_generic(GENERIC_ALL), 0x1F01FF);
            assert_eq!(
                FILE_RIGHTS.map_generic(GENERIC_READ | GENERIC_EXECUTE),
                0x1200A9
            );
            assert_eq!(FILE_RIGHTS.map_generic(GENERIC_WRITE), 0x120116);
            assert_eq!(REGISTRY_RIGHTS.map_generic(GENERIC_ALL), 0xF003F);
            assert_eq!(REGISTRY_RIGHTS.map_generic(GENERIC_READ), 0x20019);
            // Specific bits pass through untouched.
            assert_eq!(FILE_RIGHTS.map_generic(0x10000 | GENERIC_WRITE), 0x130116);
        }

        #[test]
        fn opts_select_the_target_and_view() {
            assert_eq!(parse_opts(&DynValue::Null).unwrap(), Target::File);
            assert_eq!(parse_opts(&map(&[])).unwrap(), Target::File);
            assert_eq!(
                parse_opts(&map(&[("target", s("file"))])).unwrap(),
                Target::File
            );
            assert_eq!(
                parse_opts(&map(&[("target", s("registry"))])).unwrap(),
                Target::Registry(View::Native)
            );
            assert_eq!(
                parse_opts(&map(&[("target", s("registry")), ("view", s("wow64_32"))])).unwrap(),
                Target::Registry(View::Wow64_32)
            );
        }

        #[test]
        fn opts_reject_a_file_view_and_unknown_keys() {
            let err = parse_opts(&map(&[("view", s("wow64_32"))])).unwrap_err();
            assert!(err.contains("only valid with target"), "{err}");
            let err = parse_opts(&map(&[("target", s("file")), ("view", s("native"))]));
            assert!(err.is_err());
            let err = parse_opts(&map(&[("target", s("disk"))])).unwrap_err();
            assert!(err.contains("unknown acl target"), "{err}");
            let err = parse_opts(&map(&[("follow", DynValue::Bool(true))])).unwrap_err();
            assert!(err.contains("unknown acl option 'follow'"), "{err}");
            let err = parse_opts(&map(&[("target", s("registry")), ("view", s("x"))]));
            assert!(err.unwrap_err().contains("unknown registry view"));
            assert!(parse_opts(&DynValue::Int(1)).is_err());
        }

        fn entry(pairs: &[(&str, DynValue)]) -> DynValue {
            map(pairs)
        }

        fn spec_of(entries: Vec<DynValue>) -> DynValue {
            map(&[("entries", DynValue::List(entries))])
        }

        #[test]
        fn spec_unites_rights_and_mask_and_parses_inherit_flags() {
            let spec = parse_spec(
                &spec_of(vec![entry(&[
                    ("principal", s("BUILTIN\\Users")),
                    ("kind", s("allow")),
                    ("rights", list(&["read", "synchronize"])),
                    ("mask", DynValue::Int(0x20)),
                    ("inherit", list(&["object", "container"])),
                ])]),
                Target::File,
            )
            .unwrap();
            assert_eq!(spec.owner, None);
            assert_eq!(spec.protected, None);
            assert_eq!(
                spec.entries,
                [Entry {
                    principal: "BUILTIN\\Users".to_string(),
                    kind: Kind::Allow,
                    mask: 0x1200A9,
                    flags: OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
                }]
            );
        }

        #[test]
        fn spec_aliases_and_generic_masks_normalise() {
            let spec = parse_spec(
                &spec_of(vec![
                    entry(&[
                        ("principal", s("a")),
                        ("kind", s("deny")),
                        ("rights", list(&["list_directory", "traverse"])),
                    ]),
                    entry(&[
                        ("principal", s("b")),
                        ("kind", s("allow")),
                        ("mask", DynValue::Int(GENERIC_ALL as i64)),
                    ]),
                ]),
                Target::File,
            )
            .unwrap();
            assert_eq!(spec.entries[0].mask, 0x21);
            assert_eq!(spec.entries[1].mask, 0x1F01FF);
        }

        #[test]
        fn spec_owner_and_protected_are_optional_but_entries_are_not() {
            let spec = parse_spec(
                &map(&[
                    ("owner", s("S-1-5-32-544")),
                    ("protected", DynValue::Bool(true)),
                    ("entries", DynValue::List(vec![])),
                ]),
                Target::File,
            )
            .unwrap();
            assert_eq!(spec.owner.as_deref(), Some("S-1-5-32-544"));
            assert_eq!(spec.protected, Some(true));
            let err = parse_spec(&map(&[("owner", s("x"))]), Target::File).unwrap_err();
            assert!(err.contains("needs 'entries'"), "{err}");
        }

        #[test]
        fn unknown_rights_list_the_targets_valid_names() {
            let bad = |target, right: &str| {
                parse_spec(
                    &spec_of(vec![entry(&[
                        ("principal", s("a")),
                        ("kind", s("allow")),
                        ("rights", list(&[right])),
                    ])]),
                    target,
                )
                .unwrap_err()
            };
            let err = bad(Target::File, "read_key");
            assert!(err.contains("unknown right 'read_key'"), "{err}");
            assert!(
                err.contains("read_and_execute") && err.contains("traverse"),
                "{err}"
            );
            let err = bad(Target::Registry(View::Native), "read_and_execute");
            assert!(err.contains("unknown right 'read_and_execute'"), "{err}");
            assert!(
                err.contains("read_key") && err.contains("execute_key"),
                "{err}"
            );
            assert!(!err.contains("traverse"), "{err}");
        }

        #[test]
        fn malformed_entries_are_errors() {
            let check = |pairs: &[(&str, DynValue)], want: &str| {
                let err = parse_spec(&spec_of(vec![entry(pairs)]), Target::File).unwrap_err();
                assert!(err.contains(want), "{err} should mention {want}");
            };
            let p = ("principal", s("a"));
            let allow = ("kind", s("allow"));
            let read = ("rights", list(&["read"]));
            check(&[allow.clone(), read.clone()], "no principal");
            check(&[p.clone(), read.clone()], "no kind");
            check(&[p.clone(), allow.clone()], "needs rights or mask");
            check(
                &[p.clone(), ("kind", s("audit")), read.clone()],
                "not allow or deny",
            );
            check(
                &[p.clone(), allow.clone(), ("mask", DynValue::Int(-1))],
                "32-bit",
            );
            check(
                &[p.clone(), allow.clone(), ("mask", DynValue::Int(0))],
                "grants no rights",
            );
            check(
                &[
                    p.clone(),
                    allow.clone(),
                    read.clone(),
                    ("inherit", list(&["all"])),
                ],
                "unknown inherit flag 'all'",
            );
            check(
                &[
                    p.clone(),
                    allow.clone(),
                    read.clone(),
                    ("inherit", list(&["inherit_only"])),
                ],
                "need object or container",
            );
            check(
                &[p.clone(), allow.clone(), read.clone(), ("who", s("x"))],
                "unknown key 'who'",
            );
            let err = parse_spec(&DynValue::List(vec![]), Target::File).unwrap_err();
            assert!(err.contains("must be a map"), "{err}");
            let err = parse_spec(
                &map(&[
                    ("entries", DynValue::List(vec![])),
                    ("dacl", DynValue::Null),
                ]),
                Target::File,
            )
            .unwrap_err();
            assert!(err.contains("unknown acl spec key 'dacl'"), "{err}");
        }

        fn ace(kind: Kind, flags: u8, mask: u32, s: &str) -> Ace {
            Ace {
                kind,
                flags,
                mask,
                sid: Some(sid(s)),
            }
        }

        #[test]
        fn acls_round_trip_through_the_binary_form() {
            let aces = vec![
                ace(Kind::Deny, 0, 0x10000, "S-1-5-21-1-2-3-1001"),
                ace(Kind::Allow, 0x3, 0x1200A9, "S-1-5-32-545"),
                ace(Kind::Allow, 0x1B, GENERIC_ALL, "S-1-3-0"),
            ];
            let bin = build_acl(&aces).unwrap();
            assert_eq!(bin[0], ACL_REVISION);
            assert_eq!(u16::from_le_bytes([bin[2], bin[3]]) as usize, bin.len());
            assert_eq!(u16::from_le_bytes([bin[4], bin[5]]), 3);
            assert_eq!(bin.len() % 4, 0);
            assert_eq!(parse_acl(&bin).unwrap(), aces);
            assert_eq!(parse_acl(&build_acl(&[]).unwrap()).unwrap(), []);
        }

        #[test]
        fn other_ace_types_are_read_but_not_written() {
            // An ACCESS_ALLOWED_OBJECT_ACE (type 5) header + mask + flags.
            let mut bin = vec![ACL_REVISION, 0, 20, 0, 1, 0, 0, 0];
            bin.extend_from_slice(&[5, 0x10, 12, 0]);
            bin.extend_from_slice(&0x100u32.to_le_bytes());
            bin.extend_from_slice(&[0, 0, 0, 0]);
            let aces = parse_acl(&bin).unwrap();
            assert_eq!(
                aces,
                [Ace {
                    kind: Kind::Other(5),
                    flags: 0x10,
                    mask: 0x100,
                    sid: None
                }]
            );
            assert!(build_acl(&aces).unwrap_err().contains("type 5"));
        }

        #[test]
        fn truncated_acls_are_errors() {
            let bin = build_acl(&[ace(Kind::Allow, 0, 1, "S-1-5-18")]).unwrap();
            assert!(parse_acl(&bin[..bin.len() - 1]).is_err());
            assert!(parse_acl(&bin[..4]).is_err());
            let mut lying = bin.clone();
            lying[4] = 2; // claims two ACEs
            assert!(parse_acl(&lying).is_err());
        }

        #[test]
        fn read_value_reports_names_rights_and_inherit_flags() {
            let desc = Descriptor {
                owner: Some(sid("S-1-5-32-544")),
                protected: false,
                dacl: Some(vec![
                    ace(Kind::Allow, 0x10, 0x1F01FF, "S-1-5-18"),
                    ace(Kind::Allow, 0x1B, GENERIC_ALL, "S-1-3-0"),
                    ace(Kind::Deny, 0, 0x10000, "S-1-5-21-9-9-9-1001"),
                ]),
            };
            let names = Names::from([
                (sid("S-1-5-32-544"), Some("BUILTIN\\Administrators".into())),
                (sid("S-1-5-18"), Some("NT AUTHORITY\\SYSTEM".into())),
                (sid("S-1-3-0"), Some("CREATOR OWNER".into())),
                (sid("S-1-5-21-9-9-9-1001"), None),
            ]);
            let v = to_value(&desc, Target::File, &names);
            let owner = v.get("owner").unwrap();
            assert_eq!(owner.get("sid"), Some(&s("S-1-5-32-544")));
            assert_eq!(owner.get("name"), Some(&s("BUILTIN\\Administrators")));
            assert_eq!(v.get("protected"), Some(&DynValue::Bool(false)));
            assert_eq!(v.get("null_dacl"), Some(&DynValue::Bool(false)));
            let Some(DynValue::List(entries)) = v.get("entries") else {
                panic!("no entries");
            };
            assert_eq!(entries.len(), 3);
            assert_eq!(entries[0].get("rights"), Some(&list(&["full_control"])));
            assert_eq!(entries[0].get("inherited"), Some(&DynValue::Bool(true)));
            assert_eq!(entries[0].get("inherit"), Some(&list(&[])));
            // GENERIC_ALL is reported as the file rights it grants.
            assert_eq!(entries[1].get("rights"), Some(&list(&["full_control"])));
            assert_eq!(entries[1].get("mask"), Some(&DynValue::Int(0x1F01FF)));
            assert_eq!(
                entries[1].get("inherit"),
                Some(&list(&["object", "container", "inherit_only"]))
            );
            assert_eq!(entries[2].get("kind"), Some(&s("deny")));
            assert_eq!(entries[2].get("name"), Some(&DynValue::Null));
            assert_eq!(entries[2].get("sid"), Some(&s("S-1-5-21-9-9-9-1001")));
        }

        #[test]
        fn principal_map_has_a_null_name_for_an_unresolved_sid() {
            let admins = sid("S-1-5-32-544");
            let names = Names::from([(admins.clone(), Some("BUILTIN\\Administrators".into()))]);
            assert_eq!(
                principal_map(&admins, &names),
                map(&[
                    ("sid", s("S-1-5-32-544")),
                    ("name", s("BUILTIN\\Administrators"))
                ])
            );
            let gone = sid("S-1-5-21-1-2-3-9999");
            let names = Names::from([(gone.clone(), None)]);
            assert_eq!(
                principal_map(&gone, &names),
                map(&[("sid", s("S-1-5-21-1-2-3-9999")), ("name", DynValue::Null)])
            );
        }

        fn users_read_spec() -> Spec<Principal> {
            resolved(
                parse_spec(
                    &map(&[(
                        "entries",
                        DynValue::List(vec![
                            entry(&[
                                ("principal", s("S-1-5-32-545")),
                                ("kind", s("allow")),
                                ("rights", list(&["read_and_execute", "synchronize"])),
                            ]),
                            entry(&[
                                ("principal", s("S-1-5-21-9-9-9-1001")),
                                ("kind", s("deny")),
                                ("rights", list(&["write"])),
                            ]),
                        ]),
                    )]),
                    Target::File,
                )
                .unwrap(),
            )
        }

        #[test]
        fn explicit_aces_put_deny_first_and_drop_duplicates() {
            let mut spec = users_read_spec();
            spec.entries.push(spec.entries[0].clone());
            let aces = explicit_aces(&spec);
            assert_eq!(
                aces,
                [
                    ace(Kind::Deny, 0, 0x116, "S-1-5-21-9-9-9-1001"),
                    ace(Kind::Allow, 0, 0x1200A9, "S-1-5-32-545"),
                ]
            );
        }

        #[test]
        fn inherit_flags_are_refused_on_a_plain_file() {
            let mut spec = users_read_spec();
            check_inherit(&spec, false, "C:\\f.txt").unwrap();
            spec.entries[1].flags = OBJECT_INHERIT_ACE;
            check_inherit(&spec, true, "C:\\dir").unwrap();
            let err = check_inherit(&spec, false, "C:\\f.txt").unwrap_err();
            assert_eq!(
                err,
                "the entry for <S-1-5-21-9-9-9-1001> has inherit flags, but 'C:\\f.txt' is a \
                 file: only directories and registry keys pass entries on"
            );
        }

        /// What `set` leaves on disk for `spec`: the explicit ACEs written,
        /// followed by whatever inherited ACEs the parent supplies.
        fn after_set(spec: &Spec<Principal>, inherited: Vec<Ace>) -> Descriptor {
            let mut dacl = explicit_aces(spec);
            if spec.protected != Some(true) {
                dacl.extend(inherited);
            }
            Descriptor {
                owner: spec.owner.as_ref().map(|p| p.sid.clone()),
                protected: spec.protected.unwrap_or(false),
                dacl: Some(dacl),
            }
        }

        fn inherited_users() -> Vec<Ace> {
            vec![
                ace(Kind::Allow, 0x10, 0x1F01FF, "S-1-5-18"),
                ace(Kind::Allow, 0x10, 0x1200A9, "S-1-5-32-545"),
            ]
        }

        #[test]
        fn diff_is_empty_after_set() {
            let spec = users_read_spec();
            let desc = after_set(&spec, inherited_users());
            assert_eq!(
                diff(&desc, &Names::new(), &spec, Target::File),
                Vec::<String>::new()
            );

            let mut spec = users_read_spec();
            spec.owner = Some(principal("S-1-5-32-544"));
            spec.protected = Some(true);
            let desc = after_set(&spec, inherited_users());
            assert_eq!(
                diff(&desc, &Names::new(), &spec, Target::File),
                Vec::<String>::new()
            );
        }

        #[test]
        fn diff_ignores_order_inherited_entries_and_generic_bits() {
            let spec = users_read_spec();
            let mut desc = after_set(&spec, inherited_users());
            desc.dacl.as_mut().unwrap().reverse();
            assert_eq!(
                diff(&desc, &Names::new(), &spec, Target::File),
                Vec::<String>::new()
            );
            // The same right stored as GENERIC_READ | GENERIC_EXECUTE.
            let dacl = desc.dacl.as_mut().unwrap();
            for a in dacl.iter_mut() {
                if a.mask == 0x1200A9 && a.flags == 0 {
                    a.mask = GENERIC_READ | GENERIC_EXECUTE;
                }
            }
            assert_eq!(
                diff(&desc, &Names::new(), &spec, Target::File),
                Vec::<String>::new()
            );
        }

        #[test]
        fn diff_reports_a_changed_right_as_missing_and_unexpected() {
            let spec = users_read_spec();
            let mut desc = after_set(&spec, inherited_users());
            desc.dacl.as_mut().unwrap()[1].mask = 0x1301BF;
            let names = Names::from([(sid("S-1-5-32-545"), Some("BUILTIN\\Users".into()))]);
            let d = diff(&desc, &names, &spec, Target::File);
            assert_eq!(
                d,
                [
                    "missing entry: allow <S-1-5-32-545> (S-1-5-32-545) rights \
                     [read_and_execute, synchronize] mask 0x1200A9 inherit []",
                    "unexpected entry: allow BUILTIN\\Users (S-1-5-32-545) rights \
                     [modify, synchronize] mask 0x1301BF inherit []",
                ]
            );
        }

        #[test]
        fn diff_compares_inherit_flags() {
            let spec = users_read_spec();
            let mut desc = after_set(&spec, vec![]);
            desc.dacl.as_mut().unwrap()[1].flags = OBJECT_INHERIT_ACE;
            let d = diff(&desc, &Names::new(), &spec, Target::File);
            assert_eq!(d.len(), 2, "{d:?}");
            assert!(d[1].ends_with("inherit [object]"), "{d:?}");
        }

        #[test]
        fn diff_reports_owner_and_protection() {
            let mut spec = users_read_spec();
            spec.owner = Some(principal("S-1-5-18"));
            spec.protected = Some(true);
            let desc = Descriptor {
                owner: Some(sid("S-1-5-32-544")),
                ..after_set(&users_read_spec(), inherited_users())
            };
            let names =
                Names::from([(sid("S-1-5-32-544"), Some("BUILTIN\\Administrators".into()))]);
            let d = diff(&desc, &names, &spec, Target::File);
            assert_eq!(
                d,
                [
                    "owner is BUILTIN\\Administrators (S-1-5-32-544), want <S-1-5-18> (S-1-5-18)",
                    "inheritance is enabled, want it disabled (protected: true)",
                ]
            );
            // Omitted owner and protected are not compared.
            let d = diff(&desc, &names, &users_read_spec(), Target::File);
            assert_eq!(d, Vec::<String>::new());
            // protected: false against a protected DACL.
            let mut spec = users_read_spec();
            spec.protected = Some(false);
            let desc = Descriptor {
                protected: true,
                ..after_set(&users_read_spec(), vec![])
            };
            assert_eq!(
                diff(&desc, &names, &spec, Target::File),
                ["inheritance is disabled, want it enabled (protected: false)"]
            );
        }

        #[test]
        fn diff_counts_inherited_entries_only_on_a_protected_dacl_spec() {
            let mut spec = users_read_spec();
            spec.protected = Some(true);
            let mut desc = after_set(&users_read_spec(), inherited_users());
            desc.protected = true;
            let d = diff(&desc, &Names::new(), &spec, Target::File);
            assert_eq!(d.len(), 2, "{d:?}");
            assert!(
                d.iter()
                    .all(|l| l.starts_with("unexpected inherited entry"))
            );
        }

        #[test]
        fn diff_reports_other_aces_and_a_null_dacl() {
            let spec = users_read_spec();
            let mut desc = after_set(&spec, vec![]);
            desc.dacl.as_mut().unwrap().push(Ace {
                kind: Kind::Other(9),
                flags: 0,
                mask: 1,
                sid: None,
            });
            assert_eq!(
                diff(&desc, &Names::new(), &spec, Target::File),
                ["unexpected entry: other of ACE type 9 rights [read_data] mask 0x1 inherit []"]
            );
            desc.dacl = None;
            let v = to_value(&desc, Target::File, &Names::new());
            assert_eq!(v.get("null_dacl"), Some(&DynValue::Bool(true)));
            assert_eq!(v.get("entries"), Some(&DynValue::List(vec![])));
            let d = diff(&desc, &Names::new(), &spec, Target::File);
            assert_eq!(d, ["the DACL is NULL, which grants everyone full access"]);
        }

        #[test]
        fn diff_against_an_empty_spec_lists_every_explicit_entry() {
            let desc = after_set(&users_read_spec(), inherited_users());
            let empty = resolved(parse_spec(&spec_of(vec![]), Target::File).unwrap());
            let d = diff(&desc, &Names::new(), &empty, Target::File);
            assert_eq!(d.len(), 2, "{d:?}");
            assert!(d.iter().all(|l| l.starts_with("unexpected entry")));
        }

        #[test]
        fn resolve_fails_on_the_first_unresolvable_principal() {
            let spec = parse_spec(
                &map(&[("owner", s("nobody")), ("entries", DynValue::List(vec![]))]),
                Target::File,
            )
            .unwrap();
            let err = spec
                .resolve(|p: &String| Err::<Principal, _>(format!("cannot resolve '{p}'")))
                .unwrap_err();
            assert_eq!(err, "cannot resolve 'nobody'");
        }

        #[cfg(windows)]
        #[test]
        fn mirrored_constants_match_the_windows_crate() {
            use windows::Win32::Security as s;
            use windows::Win32::Storage::FileSystem as fs;
            use windows::Win32::System::Registry as r;
            assert_eq!(OBJECT_INHERIT_ACE, s::OBJECT_INHERIT_ACE.0 as u8);
            assert_eq!(CONTAINER_INHERIT_ACE, s::CONTAINER_INHERIT_ACE.0 as u8);
            assert_eq!(
                NO_PROPAGATE_INHERIT_ACE,
                s::NO_PROPAGATE_INHERIT_ACE.0 as u8
            );
            assert_eq!(INHERIT_ONLY_ACE, s::INHERIT_ONLY_ACE.0 as u8);
            assert_eq!(INHERITED_ACE, s::INHERITED_ACE.0 as u8);
            assert_eq!(ACL_REVISION as u32, s::ACL_REVISION.0);
            use windows::Win32::Foundation as f;
            assert_eq!(GENERIC_READ, f::GENERIC_READ.0);
            assert_eq!(GENERIC_WRITE, f::GENERIC_WRITE.0);
            assert_eq!(GENERIC_EXECUTE, f::GENERIC_EXECUTE.0);
            assert_eq!(GENERIC_ALL, f::GENERIC_ALL.0);
            assert_eq!(FILE_RIGHTS.generic[0], fs::FILE_GENERIC_READ.0);
            assert_eq!(FILE_RIGHTS.generic[1], fs::FILE_GENERIC_WRITE.0);
            assert_eq!(FILE_RIGHTS.generic[2], fs::FILE_GENERIC_EXECUTE.0);
            assert_eq!(FILE_RIGHTS.generic[3], fs::FILE_ALL_ACCESS.0);
            assert_eq!(REGISTRY_RIGHTS.generic[0], r::KEY_READ.0);
            assert_eq!(REGISTRY_RIGHTS.generic[1], r::KEY_WRITE.0);
            assert_eq!(REGISTRY_RIGHTS.generic[2], r::KEY_EXECUTE.0);
            assert_eq!(REGISTRY_RIGHTS.generic[3], r::KEY_ALL_ACCESS.0);
        }
    }
}

pub fn module() -> Module {
    let mut m = Module::new("acl");
    m.doc(
        "Windows security descriptors (Windows only): the owner and DACL of files, \
         directories and registry keys",
    );

    m.doc_next(
        "Read the owner and DACL: {owner: {sid, name}, protected, entries: [{sid, name, \
         kind, rights, mask, inherit, inherited}]}, entries in stored order. \
         opts: null or {target: file | registry, view: native | wow64_32 | wow64_64}",
    );
    #[cfg(windows)]
    m.fn_(
        "read",
        |path: &str, opts: DynValue| -> Result<DynValue, String> {
            win::read(path, codec::parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_("read", |_: &str, _: DynValue| -> Result<DynValue, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m.doc_next(
        "Replace every explicit entry with spec.entries (deny before allow); spec: \
         {owner?, protected?, entries: [{principal, kind, rights?, mask?, inherit?}]}. \
         protected true disables inheritance and drops inherited entries; omitted owner and \
         protected are left as they are. opts as for read",
    );
    #[cfg(windows)]
    m.fn_(
        "set",
        |path: &str, spec: DynValue, opts: DynValue| -> Result<(), String> {
            win::set(path, &spec, codec::parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "set",
        |_: &str, _: DynValue, _: DynValue| -> Result<(), String> { Err(NOT_WINDOWS.to_string()) },
    );

    m.doc_next(
        "What set(path, spec, opts) would change, one line per difference; [] means \
         converged. Explicit entries compare as a set by SID, kind, mask and inherit flags",
    );
    #[cfg(windows)]
    m.fn_(
        "diff",
        |path: &str, spec: DynValue, opts: DynValue| -> Result<Vec<String>, String> {
            win::diff(path, &spec, codec::parse_opts(&opts)?)
        },
    );
    #[cfg(not(windows))]
    m.fn_(
        "diff",
        |_: &str, _: DynValue, _: DynValue| -> Result<Vec<String>, String> {
            Err(NOT_WINDOWS.to_string())
        },
    );

    m.doc_next(
        "Resolve a principal (a SID string or an account name, as in a spec) to {sid, name}: \
         name is the canonical DOMAIN\\name, or null when the SID does not resolve. An \
         account name that does not resolve is an Err",
    );
    #[cfg(windows)]
    m.fn_("lookup", |principal: &str| -> Result<DynValue, String> {
        win::lookup(principal)
    });
    #[cfg(not(windows))]
    m.fn_("lookup", |_: &str| -> Result<DynValue, String> {
        Err(NOT_WINDOWS.to_string())
    });

    m
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use super::codec::{self, Descriptor, Names, Principal, PrincipalRef, Sid, Target};
    use crate::hostapi::registry::win::split_hive;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NOT_ALL_ASSIGNED, ERROR_PATH_NOT_FOUND,
        ERROR_SUCCESS, GetLastError, HANDLE, HLOCAL, LUID, LocalFree, WIN32_ERROR,
    };
    use windows::Win32::Security::Authorization::{
        GetSecurityInfo, SE_FILE_OBJECT, SE_OBJECT_TYPE, SE_REGISTRY_KEY, SetSecurityInfo,
    };
    use windows::Win32::Security::{
        ACL, AdjustTokenPrivileges, DACL_SECURITY_INFORMATION, GetLengthSid,
        GetSecurityDescriptorControl, ImpersonateSelf, IsValidSid, LUID_AND_ATTRIBUTES,
        LookupAccountNameW, LookupAccountSidW, LookupPrivilegeValueW, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
        SE_DACL_PROTECTED, SE_PRIVILEGE_ENABLED, SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME,
        SID_NAME_USE, SecurityImpersonation, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
        TOKEN_QUERY, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
        OPEN_EXISTING, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    };
    use windows::Win32::System::Registry::{HKEY, REG_SAM_FLAGS, RegCloseKey, RegOpenKeyExW};
    use windows::Win32::System::SystemInformation::{ComputerNameNetBIOS, GetComputerNameExW};
    use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use wscript_std::DynValue;

    /// The text of a Win32 error, without the HRESULT the `Display` of
    /// `windows::core::Error` appends.
    fn message(rc: WIN32_ERROR) -> String {
        windows::core::Error::from_hresult(rc.to_hresult()).message()
    }

    /// A copy of `bytes` in u32-aligned storage, as SIDs and ACLs must be.
    fn aligned(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks(4)
            .map(|c| {
                let mut w = [0u8; 4];
                w[..c.len()].copy_from_slice(c);
                u32::from_ne_bytes(w)
            })
            .collect()
    }

    /// The file or registry key `acl` works on, closed on drop.
    enum Object {
        /// A file handle, and whether it is a directory.
        File(HANDLE, bool),
        Key(HKEY),
    }

    impl Object {
        fn handle(&self) -> HANDLE {
            match self {
                Object::File(h, _) => *h,
                Object::Key(k) => HANDLE(k.0),
            }
        }

        /// Whether entries can pass on to children: a directory or a key.
        fn container(&self) -> bool {
            match self {
                Object::File(_, dir) => *dir,
                Object::Key(_) => true,
            }
        }

        fn kind(&self) -> SE_OBJECT_TYPE {
            match self {
                Object::File(..) => SE_FILE_OBJECT,
                Object::Key(_) => SE_REGISTRY_KEY,
            }
        }
    }

    impl Drop for Object {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by `open` and is closed once.
            unsafe {
                match self {
                    Object::File(h, _) => {
                        let _ = CloseHandle(*h);
                    }
                    Object::Key(k) => {
                        let _ = RegCloseKey(*k);
                    }
                }
            }
        }
    }

    /// Open `path` with `access` (READ_CONTROL, WRITE_DAC, WRITE_OWNER).
    /// A file is opened with FILE_FLAG_OPEN_REPARSE_POINT, so a final
    /// symlink or junction is opened itself, not followed, and refused; the
    /// descriptor is then read and written through this one handle, so
    /// nothing can be swapped in between the check and the use.
    fn open(path: &str, target: Target, access: u32) -> Result<Object, String> {
        let gone = |rc: WIN32_ERROR| rc == ERROR_FILE_NOT_FOUND || rc == ERROR_PATH_NOT_FOUND;
        match target {
            Target::File => {
                let wide = HSTRING::from(path);
                // SAFETY: `wide` is NUL-terminated and outlives the call.
                let h = unsafe {
                    CreateFileW(
                        &wide,
                        access,
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                        None,
                        OPEN_EXISTING,
                        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                        None,
                    )
                }
                .map_err(|e| {
                    let rc = WIN32_ERROR::from_error(&e).unwrap_or_default();
                    if gone(rc) {
                        format!("'{path}' does not exist")
                    } else {
                        format!("opening '{path}': {}", e.message())
                    }
                })?;
                let mut obj = Object::File(h, false);
                let mut info = BY_HANDLE_FILE_INFORMATION::default();
                // SAFETY: `h` is an open file handle; `info` is a valid out-parameter.
                unsafe { GetFileInformationByHandle(h, &mut info) }
                    .map_err(|e| format!("inspecting '{path}': {}", e.message()))?;
                if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                    return Err(format!(
                        "refusing the ACL of '{path}': it is a symlink or reparse point \
                         (name its target instead)"
                    ));
                }
                if let Object::File(_, dir) = &mut obj {
                    *dir = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
                }
                Ok(obj)
            }
            Target::Registry(view) => {
                let (hive, sub) = split_hive(path)?;
                let wide = HSTRING::from(sub);
                let mut key = HKEY::default();
                // SAFETY: `wide` is NUL-terminated and outlives the call; `key`
                // is a valid out-parameter.
                let rc = unsafe {
                    RegOpenKeyExW(
                        hive,
                        PCWSTR(wide.as_ptr()),
                        Some(0),
                        REG_SAM_FLAGS(access | view.sam_flag()),
                        &mut key,
                    )
                };
                match rc {
                    ERROR_SUCCESS => Ok(Object::Key(key)),
                    rc if gone(rc) => Err(format!("'{path}' does not exist")),
                    rc => Err(format!("opening '{path}': {}", message(rc))),
                }
            }
        }
    }

    /// Memory Windows allocated for us, LocalFree'd on drop.
    struct LocalMem(*mut c_void);

    impl Drop for LocalMem {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from a LocalAlloc-ing API and is
                // freed once.
                unsafe {
                    let _ = LocalFree(Some(HLOCAL(self.0)));
                }
            }
        }
    }

    /// Copy the SID at `p` out of Windows-owned memory.
    ///
    /// # Safety
    /// `p` must point at a SID that stays valid for the call.
    unsafe fn copy_sid(p: PSID) -> Result<Sid, String> {
        // SAFETY: the caller guarantees `p`; GetLengthSid's length bounds
        // the slice once IsValidSid has accepted it.
        unsafe {
            if !IsValidSid(p).as_bool() {
                return Err("the descriptor holds an invalid SID".to_string());
            }
            let len = GetLengthSid(p) as usize;
            Sid::from_bytes(std::slice::from_raw_parts(p.0 as *const u8, len))
        }
    }

    /// The owner, protection and DACL of an open object.
    fn query(obj: &Object, path: &str) -> Result<Descriptor, String> {
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: all out-parameters are valid; `owner` and `dacl` point into
        // `sd`, which `_sd` frees after their last use below.
        let rc = unsafe {
            GetSecurityInfo(
                obj.handle(),
                obj.kind(),
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                Some(&mut sd),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(format!(
                "reading the security descriptor of '{path}': {}",
                message(rc)
            ));
        }
        let _sd = LocalMem(sd.0);
        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: `sd` is the live descriptor GetSecurityInfo returned.
        unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) }.map_err(|e| {
            format!(
                "reading the security descriptor of '{path}': {}",
                e.message()
            )
        })?;
        let owner = if owner.0.is_null() {
            None
        } else {
            // SAFETY: `owner` points into `sd`, still alive.
            Some(unsafe { copy_sid(owner) }?)
        };
        let dacl = if dacl.is_null() {
            None
        } else {
            // SAFETY: `dacl` points at an ACL inside `sd` whose header gives
            // its full size.
            let bytes =
                unsafe { std::slice::from_raw_parts(dacl as *const u8, (*dacl).AclSize as usize) };
            Some(codec::parse_acl(bytes)?)
        };
        Ok(Descriptor {
            owner,
            protected: control & SE_DACL_PROTECTED.0 != 0,
            dacl,
        })
    }

    /// The account name for a SID, `DOMAIN\name` or a bare `name` for the
    /// well-known SIDs with no domain (`Everyone`); None when it does not
    /// resolve.
    fn account_name(sid: &Sid) -> Option<String> {
        let buf = aligned(sid.bytes());
        let psid = PSID(buf.as_ptr() as *mut c_void);
        let (mut n, mut d) = (0u32, 0u32);
        let mut kind = SID_NAME_USE::default();
        // SAFETY: a sizing call: null buffers with zero lengths; `psid` is a
        // valid SID in `buf`.
        let _ = unsafe {
            LookupAccountSidW(PCWSTR::null(), psid, None, &mut n, None, &mut d, &mut kind)
        };
        if n == 0 {
            return None;
        }
        let mut name = vec![0u16; n as usize];
        let mut domain = vec![0u16; d.max(1) as usize];
        // SAFETY: the buffers are as long as the lengths passed.
        unsafe {
            LookupAccountSidW(
                PCWSTR::null(),
                psid,
                Some(PWSTR(name.as_mut_ptr())),
                &mut n,
                Some(PWSTR(domain.as_mut_ptr())),
                &mut d,
                &mut kind,
            )
        }
        .ok()?;
        let name = String::from_utf16_lossy(&name[..n as usize]);
        if d == 0 {
            Some(name)
        } else {
            Some(format!(
                "{}\\{name}",
                String::from_utf16_lossy(&domain[..d as usize])
            ))
        }
    }

    fn names(desc: &Descriptor) -> Names {
        let mut names = Names::new();
        for sid in desc.sids() {
            if !names.contains_key(sid) {
                names.insert(sid.clone(), account_name(sid));
            }
        }
        names
    }

    fn computer_name() -> Result<String, String> {
        let mut n = 0u32;
        // SAFETY: a sizing call with a null buffer.
        let _ = unsafe { GetComputerNameExW(ComputerNameNetBIOS, None, &mut n) };
        let mut buf = vec![0u16; n.max(1) as usize];
        // SAFETY: `buf` is `n` units long.
        unsafe { GetComputerNameExW(ComputerNameNetBIOS, Some(PWSTR(buf.as_mut_ptr())), &mut n) }
            .map_err(|e| format!("reading the computer name: {}", e.message()))?;
        Ok(String::from_utf16_lossy(&buf[..n as usize]))
    }

    /// The SID of an account name via LookupAccountNameW.
    fn account_sid(name: &str, principal: &str) -> Result<Sid, String> {
        let wide = HSTRING::from(name);
        let (mut cb, mut cch) = (0u32, 0u32);
        let mut kind = SID_NAME_USE::default();
        // SAFETY: a sizing call: null buffers with zero lengths.
        let probe = unsafe {
            LookupAccountNameW(
                PCWSTR::null(),
                &wide,
                None,
                &mut cb,
                None,
                &mut cch,
                &mut kind,
            )
        };
        let unresolved = |e: windows::core::Error| {
            format!("cannot resolve principal '{principal}': {}", e.message())
        };
        if cb == 0 {
            return Err(unresolved(
                probe
                    .err()
                    .unwrap_or_else(windows::core::Error::from_thread),
            ));
        }
        let mut sid = vec![0u32; (cb as usize).div_ceil(4)];
        let mut domain = vec![0u16; cch.max(1) as usize];
        // SAFETY: `sid` holds at least `cb` bytes and `domain` `cch` units.
        unsafe {
            LookupAccountNameW(
                PCWSTR::null(),
                &wide,
                Some(PSID(sid.as_mut_ptr() as *mut c_void)),
                &mut cb,
                Some(PWSTR(domain.as_mut_ptr())),
                &mut cch,
                &mut kind,
            )
        }
        .map_err(unresolved)?;
        let bytes: Vec<u8> = sid.iter().flat_map(|w| w.to_ne_bytes()).collect();
        Sid::from_bytes(&bytes)
    }

    /// The SID of a spec principal: a SID string as written, an account
    /// name through LookupAccountNameW.
    fn principal_sid(text: &str) -> Result<Sid, String> {
        match codec::classify(text)? {
            PrincipalRef::Sid(sid) => Ok(sid),
            PrincipalRef::Local(name) => {
                account_sid(&format!("{}\\{name}", computer_name()?), text)
            }
            PrincipalRef::Account(name) => account_sid(&name, text),
        }
    }

    /// Look up a spec principal: a SID string or an account name.
    fn principal(text: &str) -> Result<Principal, String> {
        let sid = principal_sid(text)?;
        let label = account_name(&sid).unwrap_or_else(|| text.to_string());
        Ok(Principal { sid, label })
    }

    pub fn lookup(text: &str) -> Result<DynValue, String> {
        let sid = principal_sid(text)?;
        let names = Names::from([(sid.clone(), account_name(&sid))]);
        Ok(codec::principal_map(&sid, &names))
    }

    pub fn read(path: &str, target: Target) -> Result<DynValue, String> {
        let obj = open(path, target, READ_CONTROL.0)?;
        let desc = query(&obj, path)?;
        Ok(codec::to_value(&desc, target, &names(&desc)))
    }

    pub fn diff(path: &str, spec: &DynValue, target: Target) -> Result<Vec<String>, String> {
        let spec = codec::parse_spec(spec, target)?.resolve(|p| principal(p))?;
        let obj = open(path, target, READ_CONTROL.0)?;
        codec::check_inherit(&spec, obj.container(), path)?;
        let desc = query(&obj, path)?;
        Ok(codec::diff(&desc, &names(&desc), &spec, target))
    }

    /// Thread impersonation of our own token, reverted on drop. Owner
    /// changes enable privileges on this thread-local token copy, so other
    /// worker threads never run with them.
    struct Impersonating;

    impl Drop for Impersonating {
        fn drop(&mut self) {
            // SAFETY: ImpersonateSelf succeeded on this thread.
            unsafe {
                let _ = RevertToSelf();
            }
        }
    }

    struct Token(HANDLE);

    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: an open token handle, closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Enable SeRestorePrivilege (set any owner) and
    /// SeTakeOwnershipPrivilege (open for WRITE_OWNER) for this thread
    /// until the guard drops.
    fn owner_privileges() -> Result<Impersonating, String> {
        let fail = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
        // SAFETY: no arguments beyond the level; reverted by the guard.
        unsafe { ImpersonateSelf(SecurityImpersonation) }
            .map_err(|e| fail("impersonating self", e))?;
        let guard = Impersonating;
        let mut handle = HANDLE::default();
        // SAFETY: `handle` is a valid out-parameter; the thread now has a token.
        unsafe {
            OpenThreadToken(
                GetCurrentThread(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                true,
                &mut handle,
            )
        }
        .map_err(|e| fail("opening the thread token", e))?;
        let token = Token(handle);
        for (name, label) in [
            (SE_RESTORE_NAME, "SeRestorePrivilege"),
            (SE_TAKE_OWNERSHIP_NAME, "SeTakeOwnershipPrivilege"),
        ] {
            let mut luid = LUID::default();
            // SAFETY: `name` is a static NUL-terminated string.
            unsafe { LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid) }
                .map_err(|e| fail(label, e))?;
            let state = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            // SAFETY: `state` is a valid TOKEN_PRIVILEGES; no previous state
            // is requested. AdjustTokenPrivileges reports a privilege the
            // token lacks only through the last error.
            unsafe {
                AdjustTokenPrivileges(token.0, false, Some(&state), 0, None, None)
                    .map_err(|e| fail(label, e))?;
                if GetLastError() == ERROR_NOT_ALL_ASSIGNED {
                    return Err(format!("{label} is not held by this process"));
                }
            }
        }
        drop(token);
        Ok(guard)
    }

    pub fn set(path: &str, spec: &DynValue, target: Target) -> Result<(), String> {
        let spec = codec::parse_spec(spec, target)?.resolve(|p| principal(p))?;
        // Privileges are only needed when the owner actually changes, which
        // is not known until the descriptor is read: a failure to enable them
        // is kept and reported only then.
        let privileges = spec.owner.as_ref().map(|_| owner_privileges());
        let mut access = READ_CONTROL.0 | WRITE_DAC.0;
        if let Some(Ok(_)) = privileges {
            access |= WRITE_OWNER.0;
        }
        let obj = open(path, target, access)?;
        codec::check_inherit(&spec, obj.container(), path)?;
        let current = query(&obj, path)?;

        let new_owner = spec
            .owner
            .as_ref()
            .filter(|o| current.owner.as_ref() != Some(&o.sid));
        if let (Some(o), Some(Err(e))) = (new_owner, &privileges) {
            return Err(format!(
                "changing the owner of '{path}' to {} needs SeRestorePrivilege and \
                 SeTakeOwnershipPrivilege: {e}",
                o.label
            ));
        }
        let protected = spec.protected.unwrap_or(current.protected);
        let acl = aligned(&codec::build_acl(&codec::explicit_aces(&spec))?);
        let mut info = DACL_SECURITY_INFORMATION
            | if protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
        let owner_buf = new_owner.map(|o| aligned(o.sid.bytes()));
        if owner_buf.is_some() {
            info |= OWNER_SECURITY_INFORMATION;
        }
        // SAFETY: `acl` and `owner_buf` are aligned, well-formed ACL and SID
        // buffers that outlive the call. Unprotected, Windows recomputes the
        // inherited entries from the parent.
        let rc = unsafe {
            SetSecurityInfo(
                obj.handle(),
                obj.kind(),
                info,
                owner_buf.as_ref().map(|b| PSID(b.as_ptr() as *mut c_void)),
                None,
                Some(acl.as_ptr() as *const ACL),
                None,
            )
        };
        drop(privileges);
        if rc != ERROR_SUCCESS {
            return Err(format!(
                "writing the security descriptor of '{path}': {}",
                message(rc)
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashMap;

        fn s(v: &str) -> DynValue {
            DynValue::String(v.into())
        }

        fn entry(principal: &str, kind: &str, rights: &[&str], inherit: &[&str]) -> DynValue {
            let list = |items: &[&str]| DynValue::List(items.iter().map(|i| s(i)).collect());
            DynValue::Map(HashMap::from([
                ("principal".to_string(), s(principal)),
                ("kind".to_string(), s(kind)),
                ("rights".to_string(), list(rights)),
                ("inherit".to_string(), list(inherit)),
            ]))
        }

        fn spec(protected: bool, entries: Vec<DynValue>) -> DynValue {
            DynValue::Map(HashMap::from([
                ("protected".to_string(), DynValue::Bool(protected)),
                ("entries".to_string(), DynValue::List(entries)),
            ]))
        }

        #[test]
        fn lookup_resolves_names_and_sid_strings_alike() {
            let admins = lookup("BUILTIN\\Administrators").unwrap();
            assert_eq!(admins.get("sid"), Some(&s("S-1-5-32-544")));
            assert_eq!(admins.get("name"), Some(&s("BUILTIN\\Administrators")));
            assert_eq!(lookup("S-1-5-32-544").unwrap(), admins);
            let system = lookup("NT AUTHORITY\\SYSTEM").unwrap();
            assert_eq!(system.get("sid"), Some(&s("S-1-5-18")));
            assert_eq!(system.get("name"), Some(&s("NT AUTHORITY\\SYSTEM")));
            // A SID nobody holds keeps its SID and has no name.
            let gone = lookup("S-1-5-21-1-2-3-9999").unwrap();
            assert_eq!(gone.get("sid"), Some(&s("S-1-5-21-1-2-3-9999")));
            assert_eq!(gone.get("name"), Some(&DynValue::Null));
            // The same Err set gives for the name.
            let err = lookup("nobody-here-cw").unwrap_err();
            assert!(
                err.starts_with("cannot resolve principal 'nobody-here-cw': "),
                "{err}"
            );
            let spec = spec(
                false,
                vec![entry("nobody-here-cw", "allow", &["read"], &[])],
            );
            let dir = tempfile::tempdir().unwrap();
            let set_err = set(dir.path().to_str().unwrap(), &spec, Target::File).unwrap_err();
            assert_eq!(set_err, err);
        }

        fn converge(path: &str, spec: &DynValue, target: Target) {
            set(path, spec, target).unwrap();
            assert_eq!(diff(path, spec, target).unwrap(), Vec::<String>::new());
        }

        #[test]
        fn set_then_diff_converges_on_a_directory_and_a_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().to_str().unwrap().to_string();
            let file = format!("{path}\\f.txt");
            std::fs::write(&file, "x").unwrap();

            let protected = spec(
                true,
                vec![
                    entry(
                        "NT AUTHORITY\\SYSTEM",
                        "allow",
                        &["full_control"],
                        &["object", "container"],
                    ),
                    entry(
                        "Administrators",
                        "allow",
                        &["full_control"],
                        &["object", "container"],
                    ),
                    entry(
                        "S-1-5-32-545",
                        "allow",
                        &["read", "synchronize"],
                        &["object", "container"],
                    ),
                ],
            );
            converge(&path, &protected, Target::File);
            let read = read(&file, Target::File).unwrap();
            let Some(DynValue::List(entries)) = read.get("entries") else {
                panic!("no entries: {read:?}");
            };
            // The file inherits exactly the directory's three entries.
            assert_eq!(entries.len(), 3, "{entries:?}");
            assert!(
                entries
                    .iter()
                    .all(|e| e.get("inherited") == Some(&DynValue::Bool(true)))
            );

            let deny = spec(false, vec![entry("S-1-5-32-545", "deny", &["delete"], &[])]);
            converge(&file, &deny, Target::File);
            let err = diff(&file, &protected, Target::File).unwrap_err();
            assert!(err.contains("is a file"), "{err}");
        }

        #[test]
        fn set_then_diff_converges_on_a_registry_key() {
            use crate::hostapi::registry::codec::View;
            let key = format!(
                "HKCU\\Software\\config-weave-test\\acl-{}",
                std::process::id()
            );
            crate::hostapi::registry::win::create_key(&key, View::Native).unwrap();
            let target = Target::Registry(View::Native);
            let want = spec(
                false,
                vec![entry(
                    "S-1-5-32-545",
                    "allow",
                    &["read_key"],
                    &["container"],
                )],
            );
            converge(&key, &want, target);
            crate::hostapi::registry::win::delete_key(&key, View::Native).unwrap();
            let err = read(&key, target).unwrap_err();
            assert!(err.contains("does not exist"), "{err}");
        }
    }
}
