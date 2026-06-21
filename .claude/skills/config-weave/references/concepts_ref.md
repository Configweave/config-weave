# config-weave — concepts

Each concept has its own page. This is the index.

- [**Playbooks**](../references/concept_playbooks.md) — playbook.wcl — plays of steps, gather blocks, vars, and the scheduling semantics.

- [**Packages**](../references/concept_packages.md) — package.wcl — resources, gatherers, tests, params, the directory layout, and concurrency classes.

- [**Resource & gatherer scripts**](../references/concept_scripts.md) — wscript entry-point contracts, the per-step check → apply → re-check lifecycle, reading params, logging.

- [**The wscript language**](../references/concept_wscript_language.md) — Statically typed, Rust-flavored scripting — values, functions, structs/enums, match, Option/Result, containers, traits, memory.

- [**wscript built-ins**](../references/concept_wscript_stdlib.md) — The prelude, container/string/Option/Result methods, the Value type, and the json/toml modules available in config-weave scripts.

- [**Host API — cross-platform**](../references/concept_hostapi.md) — The wscript modules config-weave registers everywhere: log, fs, path, shell, http, hash, archive, env, sys, data, template.

- [**Host API — Windows**](../references/concept_hostapi_windows.md) — registry, service, and com (IDispatch/WMI) — registered everywhere, runtime-erroring off Windows.

- [**Testing & the testlab**](../references/concept_testing.md) — config-weave test — test blocks, the three-run protocol, docker/vmlab backends, grouping, verify scripts, and scenarios.
