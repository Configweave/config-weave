# Shared script helpers (lib/)

_Resource, gatherer, verify and scenario scripts import shared code from a package's lib/ or the playbook's with `use`._

Common code lives in `lib/` — either a package's own (`pkgs/<pkg>/lib/`) or the
playbook's (`lib/`, visible to every package). A script imports a helper by its
file stem:

\`\`\`
use helpers            // pkgs/<pkg>/lib/helpers.ws, then <playbook>/lib/helpers.ws
use "./shared.ws"      // relative to the importing script
\`\`\`

Helper files are ordinary `.ws` scripts and may import each other. The whole
import graph compiles to a single unit of which \*\*only the entry file exports
functions\*\*, so a resource still satisfies the `check`/`apply` contract exactly
as a single-file one does — a helper cannot accidentally supply `check`.


| Resolution order for `use name` | Wins because |
| --- | --- |
| A registered host module | `use fs` always means the host API, even with a `lib/fs.ws` present |
| The importing script's own directory | `<dir>/name.ws` |
| The declaring package's `lib/` | a package can shadow a playbook-wide helper |
| The playbook's `lib/` | shared across every package |

> [!NOTE]
> **The extension is .ws**
> `use helpers` resolves `helpers.ws`. Path imports carry their own extension, so write `use "./shared.ws"` in full.

> [!NOTE]
> **Everything under lib/ is validated**
> `config-weave validate` compiles every `lib/*.ws`, imported or not — a broken helper fails validation on its own, and an error inside one is reported against that helper's file and line, not the script that imported it.

## Related

- [Resource](../references/concept_resource.md)

- [Gatherer](../references/concept_gatherer.md)

- [Host API](../references/concept_host_api.md)

- [Package](../references/concept_package.md)

- [Playbook](../references/concept_playbook.md)

[← Back to SKILL.md](../SKILL.md)
