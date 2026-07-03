# com

_host module_

Late-bound COM via IDispatch and WMI; returns ComObject handles; STA initialised by the engine.

`use com` — late-bound COM via IDispatch. Registered everywhere; off Windows its calls return runtime errors.

| function | signature | notes |
| --- | --- | --- |
| `create` | `(progid) -> Result[ComObject, string]` | e.g. `"WScript.Shell"` |
| `get_object` | `(name) -> Result[ComObject, string]` | moniker or running object |
| `wmi_query` | `(query) -> Result[Value, string]` | runs against `root\cimv2`; returns a **list of property maps** |

### ComObject

`ComObject` is an opaque handle with methods `get` / `get_object` / `set` / `call(name, args: List[Value])` / `call_object` / `items()`. VT_DISPATCH results must come through \`get_object`/`call_object`/`items()` because the dynamic `Value\` cannot hold an object handle.

```rust
use com
use value

fn gather(params: Value) -> Result[Value, string] {
    com::wmi_query("SELECT Name, State FROM Win32_Service WHERE StartMode='Auto'")
}
```

Worker threads are COM (STA) initialised by the engine — scripts never deal with COM lifetime or apartment setup.

## Related

- [Host API](../references/concept_host_api.md)

- [registry](../references/entity_registry_module.md)

- [Value](../references/entity_value_type.md)

[← Back to SKILL.md](../SKILL.md)
