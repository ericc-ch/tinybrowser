# 05: Stealth is later

Type: interview

Question: Is canonical-Chrome wire behavior (TLS/h2/headers) a real milestone or a guilt clause?

Answer:

Real, and **later later**. Not the next implementation effort. Not abandoned.

Keep the existing `net` public types so a btls/h1/h2 swap does not leak ureq. Do not start that stack in this charter’s follow-on coding. v1 stays ureq + native-tls. Sites that fingerprint TLS (the old live-gate “Akamai class” rows) stay failed until that milestone. Header case and HTTP/2 impersonation wait with it.
