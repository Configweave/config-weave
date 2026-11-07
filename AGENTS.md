# AGENTS.md

Instructions for agents that work on this repo in Terrarium (project
`config-weave`). Read `CLAUDE.md` for the project context and layout. Where
`CLAUDE.md` and this file disagree about the workflow (the aciddog board,
`.tree/` worktrees, direct commits to `main`), this file wins.

## Validation recipe

One command validates a change. It is the same gate that GitHub CI runs:

```
just ci::check
```

It runs `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`,
`cargo test` and `cargo test -p weave-docjson` (`.just/ci/mod.just`). It needs
no sibling checkout, no vmlab and no network beyond the cargo git
dependencies.

- Target: `config-weave-helios-build`. The sandbox has no compilers.
  `terrarium exec config-weave-helios-build -- sh -c 'cd <your checkout>/config-weave && just ci::check'`
- There is no affected-test selector: the Rust suite runs whole. A single
  part can be run while you work (`just ci::lint`, `just ci::fmt-check`,
  `just ci::test`), but the move to `review` needs the full `just ci::check`.
- The testlab suites (`just test-lab`, `test-lab-vm`, `test-ad`, `test-pkgs`)
  need vmlab, KVM and cross. They are not part of the gate. Run one only when
  the card asks for it and the target can run it.
- A run longer than a minute: use `terrarium run`, then end your turn.

## Flow (the `delivery-process` skill)

1. Claim the card from `ready` (`terrarium card claim <id> --eta <d>`).
2. Branch from `next`: `git checkout -b <card-id-lowercase> origin/next`.
   A bug card of a release branches from `release/<X.Y.Z>`.
3. Commit with conventional subjects. Push the branch. Never push to `main`,
   `next` or a `release/*` branch.
4. Run `just ci::check` on the target against the pushed commit.
5. Green: move the card to `review` with the branch, the commit, the command,
   the exit code and the test pass count. Red: fix it. A red you cannot fix
   goes to `blocked` with the output tail.
6. The lead reviews and moves the card to `merge_queue`. The merge job merges
   the whole column onto `next`, runs `just ci::check` on the result, and moves
   green cards to `accepted`.
7. Releases: the lead cuts `release/<X.Y.Z>` from `next` and merges it into
   `main` with a `pre-release: true` trailer (CI publishes `vX.Y.Z-alpha`). A
   full release (`release: true`) needs the operator's approval.

GitHub CI (`.github/workflows/ci.yml`) is not checked during card work.
