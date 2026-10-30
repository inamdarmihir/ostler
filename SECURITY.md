# Security

Waymarker lets an AI agent drive a real browser and stores some user data. Treat it like any tool that can act on your behalf.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting for this repository ("Security" tab → "Report a vulnerability"). Do not open a public issue for something exploitable. There is no bounty. This is a pre-1.0 project maintained by individuals; we will acknowledge reports as quickly as we can.

## Threat model

| threat | what we do | what we do not do |
|---|---|---|
| **Prompt injection from a web page** | Page-derived text is wrapped in `<page_content untrusted="true">`; server instructions tell the model not to follow it. | We cannot stop a model from being persuaded. Use `--allow-domain` and `--require-confirm-submit` for anything sensitive. |
| **Agent navigates somewhere it should not** | Domain allowlist enforced on navigation and after every action; only http(s); `file:` needs `--allow-file`. | The allowlist is not a sandbox against a compromised browser. |
| **Irreversible submissions** | `--require-confirm-submit` makes `form_submit` fail without `confirm=true`. | Off by default. |
| **Secrets in memory** | The memory API refuses password/token/card-like keys and card-number-like values; sensitive fields are never auto-filled from memory nor stored by `--learn`; sensitive values are masked in tool output; playbooks store structure only, never values. | Detection is heuristic (names, `autocomplete` tokens, digit patterns). A secret stored under an innocuous key with an innocuous value will not be caught. |
| **Cross-user data exposure** | Every memory point carries a `tenant`; every query filters on it; isolation is tested on a real Qdrant. | Tenants share collections. If you need hard isolation, use separate clusters or API keys. |
| **Malicious page attacking the runtime** | The injected runtime avoids `eval` and never executes page-supplied strings; it works on DOM objects. | The runtime shares a JS world with the page, so a hostile page could tamper with it (e.g. to mislead reads). Do not rely on it for adversarial pages. |
| **Local exposure** | Chrome is launched with a throwaway profile and a random debugging port bound to localhost. | Other local processes running as the same user can reach that port. |

Not audited. No security review has been done by a third party.

## What is stored, where

- Qdrant (your cluster): facts you saved, form playbooks (field labels and which fact filled them), episodes (error messages from forms), all tagged with your tenant.
- Disk: nothing by default; a Chrome profile only if you pass `--profile-dir`.
- Telemetry: none.

`waymarker memory wipe --yes` deletes everything for a tenant.
