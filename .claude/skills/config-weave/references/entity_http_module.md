# http

_host module_

HTTP client (rustls; no system TLS dependency); opts is required; returns HttpResponse.

`use http` — HTTP client (rustls; no system TLS dependency). opts is required: `Value::Null` or a map of `headers` (map), `timeout` (secs), `redirects` (bool, default true).

| function | signature |
| --- | --- |
| `get` | `(url, opts) -> Result[HttpResponse, string]` |
| `post` | `(url, body: string, opts) -> Result[HttpResponse, string]` |
| `download` | `(url, dest_path, opts) -> Result[int, string]` — returns byte count |

### HttpResponse

```rust
struct HttpResponse { status: int, body: string, headers: Map[string, string] }
```

## Related

- [Host API](../references/concept_host_api.md)

- [Value](../references/entity_value_type.md)

- [archive](../references/entity_archive_module.md)

[← Back to SKILL.md](../SKILL.md)
