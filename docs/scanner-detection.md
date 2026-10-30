# Scanner detection

Scanner detection is an opt-in path classification feature for automated
security probes. It is separate from referer-spam detection and does not alter
the normal hit, page, file, or visit totals.

Enable it in the AWStats-style configuration:

```ini
LoadPlugin="detectscanner"
ScannerRules="scanner.rules"
```

Rules are tab-separated and use the form:

```text
path_exact<TAB>/path<TAB>label
path_prefix<TAB>/path<TAB>label
path_suffix<TAB>/path<TAB>label
```

The first matching rule wins. Classified requests are aggregated under the
`scanners` report dimension and shown as **Security probes** on the Pages
detail page with red visual emphasis. Rules are intentionally explicit;
ordinary `404` responses and high request rates are not classified
automatically.
