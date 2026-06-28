# wscript string methods

Strings are immutable and char-indexed (not bytes); operations return new strings.

`len bytes_len is_empty split trim trim_start trim_end to_upper to_lower starts_with ends_with contains find replace repeat pad_left pad_right chars slice parse_int parse_float`

`find -> Option[int]`; `parse_int`/`parse_float -> Result[…, string]`; concatenate with `+`.

## Related

- [wscript: containers and strings](../references/concept_wscript_containers.md)

[← Back to SKILL.md](../SKILL.md)
