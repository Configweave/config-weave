# config-weave — glossary

| Term | Definition | Aliases |
| --- | --- | --- |
| Playbook | A directory holding playbook.wcl (plus optional lib/ and pkgs/) describing the desired state of a system. The unit config-weave checks or applies. |  |
| Play | A named group of steps inside a playbook. Steps run in parallel by default (parallel = false forces declaration order). `check`/`apply` target one play. | plays |
| Step | One unit of work in a play. Names a resource and supplies properties; carries an optional condition, requires (ordering), and concurrency tightening. |  |
| Container | A grouping block inside a play (nestable) for organisation/docs; a condition on a container applies to all its child steps. Not to be confused with a docker container. |  |
| Resource | A declared unit of desired state in a package, implemented by a wscript script exporting check() and apply(). Referenced from steps as pkg.resource. |  |
| Gatherer | A package-declared fact collector implemented by a wscript script exporting gather(params) -> Value. A playbook gather block runs one and binds its result to a variable. |  |
| Verify script | A wscript script exporting verify(facts) -> bool, run inside a test instance after apply to make custom assertions about converged state. | verify |
| Package | A bundle of resources, gatherers and tests under pkgs/<name>/. Its name qualifies refs from playbooks (core.file_present). |  |
| Gather block | A playbook block (gather "label" { from = pkg.gatherer }) that invokes a gatherer; the label becomes the variable holding the result. All gathers run concurrently before steps. |  |
| Concurrency class | A resource's scheduling restriction: parallel (no restriction), exclusive (one step of this resource type at a time), or global (runs completely alone). A step may tighten but never loosen it. |  |
| Convergence contract | The check → apply → re-check rule: check never mutates; after a successful apply, a re-check (including in a fresh process) must return AlreadyConfigured. | check → apply → re-check |
| Testlab | config-weave test — runs package tests in disposable docker containers or vmlab VMs, proving convergence with the three-run protocol (src/testlab/). |  |
| Three-run protocol | The test sequence check, apply, apply (all --json --continue-on-error). Run 2 proves in-process convergence; run 3 proves cross-process idempotence. |  |
| Scenario | A scripted, multi-stage test over a declared vmlab lab — a wscript driver brings VMs up by name, applies config-weave, reboots, and asserts. For convergence the three-run protocol can't express (e.g. a Windows DC promotion). |  |
| wscript | The statically typed, Rust-flavored scripting language resources, gatherers and verify scripts are written in. Single-file in v1, compiled against the config-weave host API. |  |
| Host API | The wscript module surface config-weave registers for scripts (log, fs, path, shell, http, hash, archive, env, sys, data, template, and Windows registry/service/com). Exactly what config-weave wscripti emits. |  |
