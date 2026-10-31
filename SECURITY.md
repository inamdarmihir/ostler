# Security

Waymarker lets an AI agent drive a real browser and stores some user data. Treat it like any tool that can act on your behalf.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting for this repository ("Security" tab → "Report a vulnerability"). Do not open a public issue for something exploitable. There is no bounty. This is a pre-1.0 project maintained by individuals; we will acknowledge reports as quickly as we can.

## Threat model

| threat | what we do | what we do not do |
|---|---|---|
| **Prompt injection from a web page** | Page-derived text is wrapped in `<page_content untrusted="true">`; server instructions tell the model not to follow it. | We cannot stop a model from being persuaded. Use `--allow-domain` and `--require-confirm-submit` for anything sensitive. |
| **Agent navigates somewhere it should not** | Domain allowlist enforced inside the browser on navigations, form posts, `fetch`/XHR, beacons and event streams; only http(s); `file:` needs `--allow-file`. | Images, scripts, stylesheets and WebSockets are not intercepted; ports are ignored; an empty allowlist permits localhost, private ranges and cloud metadata addresses. Not a sandbox against a compromised browser. |
| **Irreversible submissions** | `--require-confirm-submit` blocks every non-GET request unless it happens inside `form_submit` with `confirm=true`. | Off by default. A GET with side effects, and WebSockets, are not covered. |
| **File exfiltration through an upload field** | none yet | `form_fill` accepts any local path for a file field. Do not run it against pages you do not trust. |
| **A remote agent over A2A** | Loopback by default; bearer token required to listen elsewhere; `Origin` and `Host` checks against browser-based attacks; per-conversation browsers; submit needs an explicit confirm. | No TLS (use a reverse proxy). All conversations share one memory tenant. Always pass `--allow-domain`. See [docs/A2A.md](docs/A2A.md). |
| **Secrets in memory** | The memory API refuses password/token/card-like keys and card-number-like values; sensitive fields are never auto-filled from memory nor stored by `--learn`; sensitive values are masked in tool output; playbooks store structure only, never values. | Detection is heuristic (names, `autocomplete` tokens, digit patterns). A secret stored under an innocuous key with an innocuous value will not be caught. |
| **Cross-user data exposure** | Every memory point carries a `tenant`; every query filters on it; isolation is tested on a real Qdrant. | Tenants share collections. If you need hard isolation, use separate clusters or API keys. |
| **Malicious page attacking the runtime** | The injected runtime avoids `eval` and never executes page-supplied strings; it works on DOM objects. | The runtime shares a JS world with the page, so a hostile page could tamper with it (e.g. to mislead reads). Do not rely on it for adversarial pages. |
| **Local exposure** | Chrome is launched with a throwaway profile and a random debugging port bound to localhost. | The port has no authentication: **any local user** (not only the same one) can drive that browser. With `--profile-dir` that includes your logged-in sessions. Moving to a pipe transport is planned. |

Not audited. No security review has been done by a third party.

## What is stored, where

- Qdrant (your cluster): facts you saved, form playbooks (field labels and which fact filled them), episodes (error messages from forms), all tagged with your tenant.
- Disk: nothing by default; a Chrome profile only if you pass `--profile-dir`.
- Telemetry: none.

`waymarker memory wipe --yes` deletes everything for a tenant.
