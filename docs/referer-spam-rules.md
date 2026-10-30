# Referer-spam rule mechanics

The AWStats `detectrefererspam` plugin is a hook, not a universal detection
database. Stats therefore uses an explicit rule file instead of inventing a
heuristic that could discard legitimate traffic.

The planned minimal rule format is one rule per line:

```text
# action<TAB>pattern<TAB>label
exclude_suffix	.example-spam.test	known spam domain
classify_exact	bad.example.test	known spam host
```

Rules are evaluated in file order. The supported matching modes are:

- `exclude_exact`: do not count the event in referer dimensions, but retain the
  request and status aggregates.
- `exclude_suffix`: match the registrable host and its subdomains.
- `classify_exact`: retain the event and count it in a dedicated spam-referrer
  dimension.
- `classify_suffix`: classify a host and its subdomains.

Matching operates on the normalized referer host, strips an optional trailing
dot, lowercases ASCII names, and ignores the referer path/query. IP-literal
referers are matched only by exact rules. Invalid rules fail configuration
loading; a malformed referer is never treated as spam.

The first matching rule wins. Classification is observable in diagnostics and
reports, while exclusion is limited to referer-derived dimensions so it cannot
silently remove traffic totals. Rule files are read during update startup and
must be reloaded explicitly by the next update.

Future extensions can add CIDR and public-suffix-aware matching, but those must
remain opt-in because suffix matching alone can over-match unrelated domains.
