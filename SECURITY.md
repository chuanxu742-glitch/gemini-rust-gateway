# Security Policy

Please do not report exposed cookies, API keys, or account details in public issues.

For vulnerabilities in this repository, use GitHub's private vulnerability reporting feature when it is available. Revoke any credential that may have been disclosed before sharing diagnostic information.

Only sanitized logs and reproducible requests with placeholder credentials should be included in reports.

The Chrome sync extension only accepts loopback gateway URLs. Keep `GEMINI_COOKIE_SYNC_ENABLED=false` on remotely exposed deployments unless the endpoint has additional access controls.
