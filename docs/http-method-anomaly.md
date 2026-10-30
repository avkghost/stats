# HTTP method anomaly detection

HTTP method anomaly detection is opt-in. It classifies methods outside an
allow-list without changing normal hit, page, file, visit, or method totals.

Enable it in the AWStats-style configuration:

```ini
LoadPlugin="detecthttpmethodanomaly"
HttpMethodAnomalyAllowedMethods=GET,HEAD,POST,PUT,PATCH,DELETE,OPTIONS,CONNECT,TRACE
```

If the allow-list is omitted, the standard methods shown above are used. Method
names are compared case-insensitively and stored using uppercase. Anomalies are
aggregated in the `method_anomalies` report dimension with labels such as
`unrecognized:PROPFIND`, and appear on the Pages detail page and in CSV/JSON
reports.

This feature detects unusual method names only. It does not infer anomalies
from status codes, request rates, or repeated requests.
