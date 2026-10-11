## Release gate: the 1.9.0 `WikiEntry.sources` break

The maintainers' semver rule forbids cutting a tag while a public-API break in pk-core, pk-store or pk-librarian is unresolved. It names the 1.9.0 `WikiEntry.sources` change (`Vec<String>` → `Vec<Source>`) as the open case.

What the code provides today, checked at `2c66c3e`:

- **Builder path.** `WikiEntry::with_sources(impl IntoIterator<Item = impl Into<Source>>)`, plus `From<String> for Source` and `From<&str> for Source`. Callers passing strings compile unchanged.
- **Wire and file path.** `Source`'s `Deserialize` accepts a bare string, a mapping with `resource`, and numbers or booleans read as text. Both v0.1 and v0.2 files load.
- **Remaining source break:** code that reads the field directly and expects `String`, for example `entry.sources[0].as_str()`. No consumer does this. prometheus-cli and forge-rs depend on pk-core, pk-store and pk-librarian, and `grep -rn '\.sources\|with_sources\|Source::'` over their sources finds nothing. The okf-v02-writer proposal already recorded that forge-rs uses no pk types.

**Conclusion offered to the maintainers:** the break is mitigated by compatibility shims for every known consumer. Only direct field readers outside the estate would be affected. Whether that counts as "resolved" for the tag rule is the maintainers' decision. This change records the evidence; it does not make the decision.

## Why `candidate_count` changed meaning instead of adding a new key

The old meaning, "entries inspected (capped)", described the defect itself: a cap applied before scoring. After the fix, nothing is inspected and then dropped unscored, so the old meaning has no referent. `scored_count` reports what was inspected, and `candidate_count` keeps its name for what the cap bounds.
