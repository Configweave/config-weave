# Encrypted values

_secret("…") — password-encrypted values stored in playbook.wcl, encrypted in place by the CLI and decrypted at run time._

A value that must not sit in git in the clear is written as `secret("…")`. It is a
WCL builtin, so it works anywhere an expression does — a `vars` entry, a step's
`properties`, a gather's `params`, a `condition`:

\`\`\`wcl
vars {
  db_password = secret("hunter2")
}
\`\`\`

`config-weave secrets encrypt` rewrites each call **in place**, replacing the
literal with a `CWENC1` blob (Argon2id key derivation into XChaCha20-Poly1305).
Only the call text changes — comments, indentation and everything else in the file
are left byte-for-byte alone.

\`\`\`wcl
vars {
  db_password = secret("CWENC1.a1B2….Zx….Qm…")
}
\`\`\`

`check`, `apply` and `test` take the password from `$CONFIG_WEAVE_PASSWORD`,
`--password-stdin` or `--password-file PATH` and decrypt as the value is needed.
There is no prompt: a missing password is exit 2, so an automated run fails loudly
instead of blocking on a terminal that isn't there. A playbook with no `secret()`
calls never asks for one.


> [!WARNING]
> **An un-encrypted secret fails validation**
> `secret("plaintext")` is a hard error from `check`, `apply`, `validate`, `test` and `docs` — you cannot run, or commit, a playbook whose secrets were never encrypted. The message names `config-weave secrets encrypt` as the fix.

> [!NOTE]
> **One password per playbook**
> Adding a new secret requires the password that already unlocks the file: `secrets encrypt` decrypts every existing value before it writes anything. Use `config-weave secrets rekey` to change the password — it re-encrypts every value under a fresh salt.

> [!WARNING]
> **Playbook-only**
> `secret()` in a `package.wcl` is a validation error. Packages are shared and distributed via git, so a package cannot hold a value encrypted under one playbook's password — which also rules it out of `test` and `scenario` blocks.

> [!NOTE]
> **Decrypted values are scrubbed from output**
> Every plaintext this run decrypts is masked to `***` in diagnostics, the NDJSON log, step messages and script `log::*`/`print` output — so a resource that echoes its own password parameter does not leak it. Generated docs show `secret(…)` and never the blob.

## Related

- [Variables](../references/concept_variables.md)

- [Playbook](../references/concept_playbook.md)

- [playbook.wcl](../references/entity_playbook_wcl.md)

[← Back to SKILL.md](../SKILL.md)
