# Encrypt a value in a playbook

## Purpose

Put a password, token or key into playbook.wcl without committing it in the clear, and run against it.

## Prerequisites

- A playbook you can edit.
- A password you can supply through the environment, stdin or a file.

## Flowchart

![diagram](../_wdoc/process_encrypt_secrets-diagram-1.svg)

## Steps

### Step 1: Write the value as secret("…")

```wcl
vars {
  db_password = secret("hunter2")
}
```

> [!NOTE]
> **Anywhere an expression goes**
> `secret()` is a builtin, not a block: it is equally legal in a step's `properties`, a gather's `params` or a `condition`. Its argument must be a plain string literal — a variable or an interpolated `$"…"` cannot be encrypted in place.

Write the plaintext into the playbook. It is a validation error at this point: `check`, `apply` and `validate` all refuse to run until it is encrypted.

### Step 2: Encrypt in place

```console
$ export CONFIG_WEAVE_PASSWORD='correct horse battery staple'
$ config-weave secrets encrypt ./my-playbook
./my-playbook/playbook.wcl: encrypted 1 secret(s)
```

> [!NOTE]
> **Only the call changes**
> The rewrite splices the one call's byte range and re-parses before writing. Comments, indentation and formatting elsewhere are untouched, so the diff is one line.

Run `config-weave secrets encrypt <dir>`. Adding a second secret later needs the \*same\* password — the command decrypts every existing value first, and points at `secrets rekey` if the password does not match.

### Step 3: Check and apply with the password

```console
$ config-weave check ./my-playbook baseline          # $CONFIG_WEAVE_PASSWORD
$ config-weave apply ./my-playbook baseline --password-stdin < pw.txt
$ config-weave apply ./my-playbook baseline --password-file /run/secrets/pw
```

> [!NOTE]
> **Verified up front**
> The run decrypts every value before executing, not when the evaluator happens to reach one — so a wrong password fails immediately (exit 2) even if no step references the secret.

Supply the password through exactly one of `$CONFIG_WEAVE_PASSWORD`, `--password-stdin` or `--password-file PATH`. Giving more than one is an error rather than a silent pick.

### Step 4: Edit or re-key later

```console
$ config-weave secrets decrypt ./my-playbook       # back to plaintext, to edit
$ config-weave secrets encrypt ./my-playbook       # re-encrypt

$ CONFIG_WEAVE_NEW_PASSWORD='a new one' \
    config-weave secrets rekey ./my-playbook       # change the password
```

> [!WARNING]
> **decrypt writes plaintext to disk**
> `secrets decrypt` puts the real values back in the file so you can edit them. Re-encrypt before committing.

`rekey` decrypts with the old password, mints a fresh salt and re-encrypts everything under the new one — sweeping up any still-plaintext calls in the same pass. The new password comes from `--new-password-file` or `$CONFIG_WEAVE_NEW_PASSWORD`.

> [!TIP]
> **Verification**
> `grep` the playbook for the plaintext finds nothing, `config-weave validate` passes, and `check` with the password reports the play normally.

## Related

- [Encrypted values](../references/concept_encrypted_values.md)

- [Variables](../references/concept_variables.md)

- [Playbook](../references/concept_playbook.md)

[← Back to SKILL.md](../SKILL.md)
