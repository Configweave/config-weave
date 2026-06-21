# Ansible standard library (`ansible.builtin`) — resource survey

> **Temporary planning scratch file** — not committed. Generated 2026-06-15 from
> `ansible-core` via `ansible-doc -l ansible.builtin -j` (71 modules, the canonical
> "standard library" that ships with ansible-core; the huge `community.general` /
> `ansible.posix` collections are out of scope here).

Purpose: pick which Ansible modules to draw inspiration from when growing config-weave's
resource/gatherer library. Each entry is tagged by how it maps to config-weave's model:

- 🟢 **Resource candidate** — declarative, state-convergent; good inspiration for a
  config-weave `resource` (check/apply idempotence).
- 🟡 **Gatherer** — read-only fact collection; maps to a config-weave `gatherer`.
- 🔵 **Imperative / command** — runs commands; config-weave models desired *state*, not
  ad-hoc execution. Mostly out of scope (or a deliberately narrow escape hatch).
- ⚪ **Playbook control / meta** — flow, inventory, vars, includes; handled by
  config-weave's playbook/engine layer, not by resources.

The "CW today" column is a rough note on what config-weave-pkgs already has (from the
linux_* packages) — verify before relying on it.

---

## Files & content — 🟢 mostly resource candidates

| Module | Description | Tag | CW today |
|---|---|---|---|
| `file` | Manage files and file properties (path, mode, owner, symlink, absent) | 🟢 | ✓ `file`, `directory`, `symlink` |
| `copy` | Copy files to remote locations | 🟢 | partial (`file` w/ content) |
| `template` | Template a file out to a target host | 🟢 | gap (no templating resource) |
| `lineinfile` | Manage lines in text files | 🟢 | ✓ `line_in_file` |
| `blockinfile` | Insert/update/remove a marked text block | 🟢 | gap |
| `replace` | Regex replace all instances of a string in a file | 🟢 | gap |
| `assemble` | Assemble a file from fragments in a directory | 🟢 | gap |
| `get_url` | Download a file from HTTP/HTTPS/FTP to the node | 🟢 | gap |
| `unarchive` | Unpack an archive (optionally copy it first) | 🟢 | gap |
| `tempfile` | Create temporary files/directories | 🔵 | — (imperative) |
| `fetch` | Pull files from remote nodes to the controller | 🔵 | — (controller model n/a) |
| `slurp` | Read a remote file back as base64 | 🟡 | — |
| `stat` | Retrieve file / filesystem status | 🟡 | gap (file-fact gatherer) |
| `find` | List files matching criteria | 🟡 | gap |

## Packages & repositories — 🟢 strong fit

| Module | Description | Tag | CW today |
|---|---|---|---|
| `package` | Generic OS package manager (abstracts apt/dnf/...) | 🟢 | ✓ `package_installed` |
| `apt` | Manage apt packages | 🟢 | ✓ (via `package_installed`) |
| `dnf` / `dnf5` | Manage packages with dnf / dnf5 | 🟢 | ✓ (via `package_installed`) |
| `apt_key` | Add/remove an apt key | 🟢 | gap (deprecated upstream; see deb822) |
| `apt_repository` | Add/remove APT repositories | 🟢 | ✓ `apt_repository` |
| `deb822_repository` | Add/remove deb822-format repos (modern) | 🟢 | gap |
| `yum_repository` | Add/remove YUM repositories | 🟢 | gap |
| `rpm_key` | Add/remove a gpg key from the rpm db | 🟢 | gap |
| `dpkg_selections` | Set dpkg package selections (hold, etc.) | 🟢 | gap |
| `debconf` | Configure a .deb package's debconf answers | 🟢 | gap |
| `pip` | Manage Python library dependencies | 🟢 | gap |
| `package_facts` | Installed-package info as facts | 🟡 | ✓ (package-manager gatherer) |

## Services & init — 🟢

| Module | Description | Tag | CW today |
|---|---|---|---|
| `service` | Manage services (generic init abstraction) | 🟢 | partial (`systemd_unit`) |
| `systemd` / `systemd_service` | Manage systemd units (enable/state/daemon-reload) | 🟢 | ✓ `systemd_unit` (file); gap on enable/state |
| `sysvinit` | Manage SysV services | 🟢 | gap |
| `service_facts` | Service state as facts | 🟡 | gap |

## Users, groups, keys — 🟢

| Module | Description | Tag | CW today |
|---|---|---|---|
| `user` | Manage user accounts | 🟢 | ✓ `user` |
| `group` | Add/remove groups | 🟢 | ✓ `group` |
| `known_hosts` | Add/remove a host in known_hosts | 🟢 | gap |
| (authorized keys are `ansible.posix.authorized_key`) | — | 🟢 | ✓ `authorized_key` |

## System configuration — 🟢

| Module | Description | Tag | CW today |
|---|---|---|---|
| `cron` | Manage cron.d / crontab entries | 🟢 | ✓ `cron_file` (file-based; cron-entry is a gap) |
| `hostname` | Manage the system hostname | 🟢 | gap |
| `iptables` | Modify iptables rules | 🟢 | gap |
| `reboot` | Reboot the machine and wait for it | 🔵 | — (CW has `reboot_required` status instead) |
| `getent` | Wrapper over the unix getent database | 🟡 | gap |
| `mount_facts` | Mount information | 🟡 | gap |
| `setup` / `gather_facts` | Gather host facts | 🟡 | ✓ `os_info` gatherer (subset) |
| (sysctl is `ansible.posix.sysctl`) | — | 🟢 | ✓ `sysctl_dropin` |

## Source control / deploy — 🟢

| Module | Description | Tag | CW today |
|---|---|---|---|
| `git` | Deploy software/files from a git checkout | 🟢 | gap |
| `subversion` | Deploy a subversion working copy | 🟢 | gap |

## Network / web — mixed

| Module | Description | Tag | CW today |
|---|---|---|---|
| `uri` | Interact with web services (HTTP) | 🔵 | gap (could be a check/probe) |
| `wait_for` | Wait for a port/file/condition | 🔵 | — (runtime) |
| `wait_for_connection` | Wait until the host is reachable | 🔵 | — (runtime) |
| `ping` | Verify connectivity + usable python | 🔵 | — |

## Imperative command execution — 🔵 (out of scope or narrow escape hatch)

| Module | Description | Tag |
|---|---|---|
| `command` | Run a command (no shell) | 🔵 |
| `shell` | Run shell commands | 🔵 |
| `raw` | Low-level command over the connection | 🔵 |
| `script` | Transfer + run a local script | 🔵 |
| `expect` | Run a command and respond to prompts | 🔵 |

## Playbook control / meta — ⚪ (config-weave engine territory, not resources)

| Module | Description | Tag |
|---|---|---|
| `set_fact` | Set host vars/facts | ⚪ |
| `set_stats` | Define run stats | ⚪ |
| `include_vars` | Load vars from files at runtime | ⚪ |
| `import_playbook` / `import_role` / `import_tasks` | Static includes | ⚪ |
| `include_role` / `include_tasks` | Dynamic includes | ⚪ |
| `add_host` / `group_by` | Mutate in-memory inventory | ⚪ |
| `debug` / `assert` / `fail` | Diagnostics / assertions | ⚪ |
| `pause` | Pause execution | ⚪ |
| `meta` | Run engine actions (flush_handlers, end_play, ...) | ⚪ |
| `async_status` | Poll an async task | ⚪ |
| `validate_argument_spec` | Validate role argument specs | ⚪ |

---

## Quick planning shortlist (gaps that look worth stealing)

Highest-value 🟢 resource candidates config-weave doesn't appear to cover yet:

1. **`template`** — rendered config files (big one; pairs with the existing wscript/vars).
2. **`blockinfile`** / **`replace`** — text-block + regex file editing (complements `line_in_file`).
3. **`get_url`** + **`unarchive`** — fetch & extract (install-from-tarball flows).
4. **Repo/key family** — `deb822_repository`, `yum_repository`, `rpm_key`, `apt_key` (modernize repo management beyond `apt_repository`).
5. **`service`/`systemd` enable+state** — config-weave has the unit *file*; the
   enable/started/daemon-reload convergence is a gap.
6. **`hostname`**, **`iptables`**, **`known_hosts`**, **`cron` entry** (vs file), **`pip`**, **`git`** — common system-config gaps.
7. **Fact gatherers** — `stat`, `find`, `service_facts`, `getent` map cleanly to config-weave gatherers.
