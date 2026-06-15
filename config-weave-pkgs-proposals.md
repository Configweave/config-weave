# config-weave-pkgs — proposed resource/gatherer changes

> **Temporary planning scratch file** — not committed. Date: 2026-06-15.
> Cross-references `ansible-resource-survey.md` (the `ansible.builtin` list) against the
> *actual* current contents of `../config-weave-pkgs`. Proposals are written to
> **config-weave norms, not Ansible's** — see the next section.

## How these follow config-weave norms (not Ansible norms)

The current library already establishes the conventions; new work matches them:

1. **One resource = one concern. Split present/absent and per-facet.** Ansible overloads a
   module with a `state`/`enabled`/`masked` mega-param; config-weave instead ships focused
   resources — `package_installed` + `package_absent`, `service_state` + `service_enabled`.
   New removals/facets become their own resources, not a `state` param.
2. **Package-qualified, `snake_case`, descriptive names.** `linux_files.block_in_file`, not
   `ansible.builtin.blockinfile`. Noun or noun_qualifier.
3. **Modern backends over legacy.** Already true (nftables/firewalld over iptables). New
   repo/key work uses deb822 `.sources` and the `/etc/apt/keyrings` model, **not** the
   deprecated `apt-key`.
4. **Content is composed in WCL, not templated in the resource.** config-weave builds file
   bodies from playbook vars + `$"…"` interpolation and feeds them to a `content` param.
   So there is **no `template` resource** (Ansible's `template`/Jinja) — that's a
   deliberate divergence, covered by `file`/`*_file` + WCL.
5. **Declared concurrency.** `parallel` for independent files, `exclusive` for
   package-manager / single-tool locks, `global` for whole-system single files. User-scoped
   resources take a `home` param (default `""` = current user), as KDE/tmux already do.
6. **Static, declarative inputs.** No network-scan or prompt-driven inputs (no
   `ssh-keyscan`, no `expect`); the desired value is passed in.
7. **Gatherers are read-only facts; resources never gather.** check/apply return
   `AlreadyConfigured`/`NotConfigured` and converge idempotently (read → diff → write).
8. **Cross-distro via a `manager`/auto-detect param** where relevant (as `package_installed`
   already does).

---

## Updates to existing resources (small, additive)

| Package.resource | Proposed change | Inspired by | Notes |
|---|---|---|---|
| `linux_packages.package_installed` | add optional `version` (string, `""`) to pin a version | apt/dnf version= | keep `manager=auto`; empty = any version (today's behaviour) |
| `linux_files.download` | add optional `owner`/`group` (string, `""`) for parity with `file` | get_url `owner/group` | already has `sha256`,`mode` |
| `linux_files.line_in_file` | add a sibling `linux_files.line_absent` (don't add a `state` param) | lineinfile `state=absent` | present/absent split per norm #1 |
| `linux_services.systemd_unit` | document/verify it pairs with `service_enabled`+`service_state` | systemd enabled/state | facets already exist as separate resources ✔ |

---

## New resources to add (by package)

### linux_files

| Proposed resource | Purpose | Key params | Concurrency | Inspired by |
|---|---|---|---|---|
| `path_absent` | Remove a file/dir/symlink at a path | `path` (req); `recursive` (bool, `false`) | parallel | `file state=absent` |
| `block_in_file` | Ensure a marked text block exists/updated between auto markers | `path` (req); `block` (req); `marker_label` (string, `"config-weave"`); `create` (bool, `true`) | parallel | `blockinfile` |
| `block_absent` | Remove a previously-managed marked block | `path` (req); `marker_label` (string, `"config-weave"`) | parallel | `blockinfile state=absent` |
| `file_substitution` | Regex-replace occurrences in a file | `path` (req); `pattern` (req, regex); `replacement` (req) | parallel | `replace` |
| `archive_extracted` | Ensure an archive is unpacked into a dir | `src` (req, local path); `dest` (req); `creates` (string, marker for idempotence) | parallel | `unarchive` (local only; no remote-copy mode) |

### linux_network

| Proposed resource | Purpose | Key params | Concurrency | Inspired by |
|---|---|---|---|---|
| `known_host` | Ensure a host key line in a known_hosts file | `name` (req, host); `key` (req, public key line); `home` (string, `""`); `path` (string, `""` → `~/.ssh/known_hosts`) | parallel | `known_hosts` (key passed in, **no** ssh-keyscan) |

### linux_packages

| Proposed resource | Purpose | Key params | Concurrency | Inspired by |
|---|---|---|---|---|
| `apt_repository_deb822` | Modern deb822 `.sources` file in `/etc/apt/sources.list.d` | `name` (req); `content` (req) | exclusive | `deb822_repository` |
| `yum_repository` | `.repo` file in `/etc/yum.repos.d` | `name` (req); `content` (req) | exclusive | `yum_repository` |
| `apt_signing_key` | Install a gpg key under `/etc/apt/keyrings/<name>` | `name` (req); `content` (req, armored); `dearmor` (bool, `true`) | exclusive | `apt_key` (modern keyring model, **not** apt-key) |
| `rpm_gpg_key` | Import a gpg key into the rpm db | `name` (req); `content` (req, armored) | exclusive | `rpm_key` |
| `package_held` / `package_unheld` | Hold/unhold a package version (apt-mark / dnf versionlock / dpkg selections) | `name` (req); `manager` (string, `"auto"`) | exclusive | `dpkg_selections` |
| `debconf_selection` | Pre-seed a debconf answer | `package` (req); `question` (req); `vtype` (string, `"string"`); `value` (req) | exclusive | `debconf` |

### linux_system

| Proposed resource | Purpose | Key params | Concurrency | Inspired by |
|---|---|---|---|---|
| `cron_job` | Manage a single crontab entry by name marker (vs whole-file `cron_file`) | `name` (req, id); `schedule` (req); `command` (req); `user` (string, `"root"`) | parallel | `cron` |
| `cron_job_absent` | Remove a named crontab entry | `name` (req); `user` (string, `"root"`) | parallel | `cron state=absent` |

### New package: `linux_scm`

| Proposed resource | Purpose | Key params | Concurrency | Inspired by |
|---|---|---|---|---|
| `git_checkout` | Ensure a git repo present at a ref | `repo` (req, url); `dest` (req); `ref` (string, `""`); `depth` (int, `0`); `force` (bool, `false`) | parallel | `git` |
| `subversion_checkout` (lower priority) | Ensure an svn working copy | `repo` (req); `dest` (req); `revision` (string, `""`) | parallel | `subversion` |

### New package: `linux_python`

> Language-level packages, kept out of `linux_packages` (which is OS-level). Could
> alternatively live there as `pip_package`/`pip_absent` — flag for decision.

| Proposed resource | Purpose | Key params | Concurrency | Inspired by |
|---|---|---|---|---|
| `pip_package` | Ensure a pip package installed (system or venv) | `name` (req); `version` (string, `""`); `virtualenv` (string, `""`); `executable` (string, `"pip3"`) | exclusive | `pip` |
| `pip_absent` | Ensure a pip package removed | `name` (req); `virtualenv` (string, `""`); `executable` (string, `"pip3"`) | exclusive | `pip state=absent` |

---

## New gatherers to add

| Package.gatherer | Purpose | Params | Inspired by |
|---|---|---|---|
| `linux_files.path_stat` | Report a path's existence/type/mode/owner/size | `path` (req) | `stat` |
| `linux_facts.services` | Map of services → state/enabled | — | `service_facts` |
| `linux_facts.mounts` | Mounted filesystems | — | `mount_facts` |
| `linux_facts.getent` | Look up a passwd/group/hosts/etc. database key | `database` (req); `key` (string, `""`) | `getent` |
| `linux_files.paths_matching` (lower priority) | List files matching criteria under a root | `path` (req); `pattern` (string, `"*"`) | `find` |

---

## Deliberately NOT porting (Ansible → config-weave divergences)

| Ansible module(s) | Why not | config-weave equivalent |
|---|---|---|
| `template` | Content is composed in WCL (vars + `$"…"`), not Jinja in a resource | `file` / `*_file` + WCL interpolation |
| `command`, `shell`, `raw`, `script`, `expect` | config-weave models desired **state**, not ad-hoc execution | resources; (optional, contentious: a guarded `command_run` with `creates`/`unless` — flag for discussion) |
| `copy`, `fetch`, `slurp` | No controller→node copy model; reading is a gatherer concern | `file` (content), `download`, `path_stat` |
| `iptables` | Superseded by modern backend already shipped | `linux_network.nftables_ruleset`, `firewalld_service` |
| `apt_key` | Deprecated upstream | proposed `apt_signing_key` (keyrings) |
| `package` (generic) | Already covered with auto-detect | `package_installed` (`manager=auto`) |
| `reboot` | A runner action, not a convergent resource | engine surfaces the `reboot_required` step status |
| `set_fact`, `include_*`, `import_*`, `debug`, `assert`, `fail`, `pause`, `meta`, `add_host`, `group_by`, `gather_facts`, `async_status`, `validate_argument_spec`, `ping`, `wait_for*` | Playbook control / runtime — handled by the engine, WCL vars, `condition`/`requires`, and gatherers | (no resource) |

---

## Suggested priority / roadmap

**Tier 1 — high value, fills obvious gaps, low design risk**
- `linux_files.path_absent`, `line_absent` (removal parity — currently no way to *remove*).
- `linux_files.block_in_file` / `block_absent` (the most-requested file-editing primitive after lineinfile).
- `linux_packages.apt_signing_key` + `apt_repository_deb822` + `yum_repository` + `rpm_gpg_key` (modern repo onboarding; today only legacy `apt_repository` exists).
- `linux_scm.git_checkout` (deploy-from-git is a staple).

**Tier 2 — useful, slightly more design**
- `linux_files.file_substitution`, `archive_extracted`.
- `linux_system.cron_job` / `cron_job_absent` (granular vs whole-file).
- `linux_python.pip_package` / `pip_absent`.
- `linux_network.known_host`.
- Gatherers: `path_stat`, `linux_facts.services`.

**Tier 3 — nice to have / decide later**
- `package_held`/`debconf_selection`, `subversion_checkout`, `getent`/`mounts`/`paths_matching` gatherers.
- Decision needed: a guarded `command_run` escape hatch — yes/no?

**Open decisions to confirm before building**
1. New packages `linux_scm` and `linux_python` vs folding into existing ones?
2. Removal model confirmed as separate `*_absent` resources (vs a `state` param)? (current norm says separate.)
3. Any appetite for a guarded `command_run`?
